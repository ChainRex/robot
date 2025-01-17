use axum::extract::Extension;
use axum::{
    extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
    response::Response,
    routing::get,
    serve, Router,
};
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::sync::mpsc;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::broadcast;
use tokio::sync::Mutex;
use tokio::time::{self, Duration};
use tower_http::services::ServeDir;

// 定义消息类型
#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type", content = "data")]
enum Message {
    #[serde(rename = "register")]
    Register(RegisterData),
    #[serde(rename = "control_command")]
    ControlCommand(ControlCommandData),
    #[serde(rename = "image_data")]
    ImageData(ImageData),
    #[serde(rename = "image_fragment")]
    ImageFragment(ImageFragment),
    #[serde(rename = "heartbeat")]
    Heartbeat(HeartbeatData),
}

#[derive(Serialize, Deserialize, Debug)]
struct RegisterData {
    client_type: ClientType,
    client_id: String, // 用于区分多个客户端
}

#[derive(Serialize, Deserialize, Debug)]
struct ControlCommandData {
    command: Command,
    timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct ImageData {
    image: String, // Base64 编码的图像数据
    timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct ImageFragment {
    sequence: u32, // 当前分片编号，从1开始
    total: u32,    // 总分片数
    image: String, // 当前分片的图像数据
    timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug)]
struct HeartbeatData {
    client_type: ClientType,
    client_id: String,
    timestamp: u64,
}

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Hash, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum ClientType {
    Control,
    Robot,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "lowercase")]
enum Command {
    Forward,
    Backward,
    Left,
    Right,
}

#[derive(Clone, Debug)]
struct LogMessage {
    message: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Create a channel for log messages
    let (log_tx, log_rx) = mpsc::channel::<LogMessage>();
    let log_tx = Arc::new(Mutex::new(log_tx));

    // Create the UDP socket and start the UDP server
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:8080").await?);
    println!("UDP服务器已启动，监听地址 0.0.0.0:8080");

    let (broadcast_tx, _) = broadcast::channel::<String>(100);
    let broadcast_tx = Arc::new(broadcast_tx);

    let mut client_manager = ClientManager::new();
    client_manager.broadcast_tx = Some(broadcast_tx.clone());
    let clients = Arc::new(Mutex::new(client_manager));

    // Start the web server
    let app = Router::new()
        .route("/ws", get(ws_handler))
        .nest_service("/", ServeDir::new("static"))
        .layer(Extension(socket.clone()))
        .layer(Extension(clients.clone()));

    let log_tx_clone = log_tx.clone();
    println!("Web服务器已启动，访问 http://localhost:3000");

    // Start the web server
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
        serve(listener, app).await.unwrap();
    });

    // 复制 Arc<UdpSocket> 而不是直接复制 UdpSocket
    let socket_clone = Arc::clone(&socket);
    let clients_clone = clients.clone();

    // 启动一个任务来接收和处理消息
    tokio::spawn(async move {
        let mut buf = [0u8; 65535];
        loop {
            match socket_clone.recv_from(&mut buf).await {
                Ok((len, addr)) => {
                    let msg_result: Result<Message, _> = serde_json::from_slice(&buf[..len]);
                    match msg_result {
                        Ok(msg) => {
                            handle_message(msg, addr, &socket_clone, &clients_clone).await;
                        }
                        Err(e) => {
                            eprintln!("解析消息失败: {}", e);
                            // 可选择发送错误消息给客户端
                        }
                    }
                }
                Err(e) => {
                    eprintln!("接收数据失败: {}", e);
                }
            }
        }
    });

    // 启动一个任务来监控心跳并清理失效的客户端
    let clients_clone = clients.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let mut clients = clients_clone.lock().await;
            clients.cleanup();
        }
    });

    // 防止主任务退出
    loop {
        time::sleep(Duration::from_secs(60)).await;
    }
}

// 客户端管理器
struct ClientManager {
    control_clients: HashMap<String, std::net::SocketAddr>,
    robot_clients: HashMap<String, std::net::SocketAddr>,
    last_heartbeat: HashMap<(ClientType, String), u64>,
    broadcast_tx: Option<Arc<broadcast::Sender<String>>>,
}

impl ClientManager {
    fn new() -> Self {
        ClientManager {
            control_clients: HashMap::new(),
            robot_clients: HashMap::new(),
            last_heartbeat: HashMap::new(),
            broadcast_tx: None,
        }
    }

    async fn send_log(&self, msg: String) {
        println!("{}", msg);
        if let Some(tx) = &self.broadcast_tx {
            let _ = tx.send(
                serde_json::json!({
                    "message": msg
                })
                .to_string(),
            );
        }
    }

    async fn register(
        &mut self,
        client_type: ClientType,
        client_id: String,
        addr: std::net::SocketAddr,
    ) {
        match client_type {
            ClientType::Control => {
                self.control_clients.insert(client_id.clone(), addr);
                self.send_log(format!("注册控制端: ID = {}, 地址 = {}", client_id, addr))
                    .await;
            }
            ClientType::Robot => {
                self.robot_clients.insert(client_id.clone(), addr);
                self.send_log(format!("注册机器人端: ID = {}, 地址 = {}", client_id, addr))
                    .await;
            }
        }
        self.last_heartbeat
            .insert((client_type, client_id), current_timestamp());
    }

    fn update_heartbeat(&mut self, client_type: ClientType, client_id: String, timestamp: u64) {
        let client_id_clone = client_id.clone();
        self.last_heartbeat
            .insert((client_type, client_id), timestamp);
        self.send_log(format!(
            "收到心跳: 类型 = {:?}, ID = {}, 时间戳 = {}",
            client_type, client_id_clone, timestamp
        ));
    }

    fn get_robot_clients(&self) -> Vec<std::net::SocketAddr> {
        self.robot_clients.values().cloned().collect()
    }

    fn get_control_clients(&self) -> Vec<std::net::SocketAddr> {
        self.control_clients.values().cloned().collect()
    }

    fn cleanup(&mut self) {
        let threshold = current_timestamp() - 60;
        let keys: Vec<_> = self
            .last_heartbeat
            .iter()
            .filter(|&(_, &ts)| ts < threshold)
            .map(|(k, _)| k.clone())
            .collect();

        for key in keys {
            let (client_type, client_id) = key;
            match client_type {
                ClientType::Control => {
                    if let Some(addr) = self.control_clients.remove(&client_id) {
                        self.send_log(format!("控制端超时: ID = {}, 地址 = {}", client_id, addr));
                    }
                }
                ClientType::Robot => {
                    if let Some(addr) = self.robot_clients.remove(&client_id) {
                        self.send_log(format!("机器人端超时: ID = {}, 地址 = {}", client_id, addr));
                    }
                }
            }
            self.last_heartbeat.remove(&(client_type, client_id));
        }
    }
}

async fn handle_message(
    msg: Message,
    addr: std::net::SocketAddr,
    socket: &Arc<UdpSocket>,
    clients: &Arc<Mutex<ClientManager>>,
) {
    let mut clients_guard = clients.lock().await;
    match msg {
        Message::Register(data) => {
            let client_type = data.client_type;
            let client_id = data.client_id;
            clients_guard.register(client_type, client_id, addr).await;
        }
        Message::ControlCommand(data) => {
            clients_guard
                .send_log(format!(
                    "收到控制指令: {:?}, 时间戳 = {}",
                    data.command, data.timestamp
                ))
                .await;
            let serialized = serde_json::to_vec(&Message::ControlCommand(data)).unwrap();
            let robot_clients = clients_guard.get_robot_clients();
            for robot_addr in robot_clients {
                if let Err(e) = socket.send_to(&serialized, robot_addr).await {
                    clients_guard
                        .send_log(format!("转发控制指令到 {} 失败: {}", robot_addr, e))
                        .await;
                } else {
                    clients_guard
                        .send_log(format!("转发控制指令到 {}", robot_addr))
                        .await;
                }
            }
        }
        Message::ImageData(data) => {
            clients_guard
                .send_log(format!("收到图像数据, 时间戳 = {}", data.timestamp))
                .await;
            let serialized = serde_json::to_vec(&Message::ImageData(data)).unwrap();
            let control_clients = clients_guard.get_control_clients();
            for control_addr in control_clients {
                if let Err(e) = socket.send_to(&serialized, control_addr).await {
                    clients_guard
                        .send_log(format!("转发图像数据到 {} 失败: {}", control_addr, e))
                        .await;
                } else {
                    clients_guard
                        .send_log(format!("转发图像数据到 {}", control_addr))
                        .await;
                }
            }
        }
        Message::ImageFragment(data) => {
            clients_guard
                .send_log(format!(
                    "收到图像分片: {}/{}, 时间戳 = {}",
                    data.sequence, data.total, data.timestamp
                ))
                .await;
            let serialized = serde_json::to_vec(&Message::ImageFragment(data)).unwrap();
            let control_clients = clients_guard.get_control_clients();
            for control_addr in control_clients {
                if let Err(e) = socket.send_to(&serialized, control_addr).await {
                    clients_guard
                        .send_log(format!("转发图像分片到 {} 失败: {}", control_addr, e))
                        .await;
                } else {
                    clients_guard
                        .send_log(format!("转发图像分片到 {}", control_addr))
                        .await;
                }
            }
        }
        Message::Heartbeat(data) => {
            clients_guard.update_heartbeat(data.client_type, data.client_id, data.timestamp);
        }
    }
}

// 获取当前 UNIX 时间戳（秒）
fn current_timestamp() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

// Add this new function for handling WebSocket connections
async fn ws_handler(
    ws: WebSocketUpgrade,
    socket: Extension<Arc<UdpSocket>>,
    clients: Extension<Arc<Mutex<ClientManager>>>,
) -> Response {
    let socket = socket.0;
    let clients = clients.0;

    ws.on_upgrade(move |websocket| async move {
        println!("WebSocket 客户端已连接");
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
        let (mut sender, mut receiver) = websocket.split();

        while let Some(msg) = receiver.next().await {
            if let Ok(msg) = msg {
                match msg {
                    WsMessage::Text(text) => {
                        if let Ok(udp_msg) = serde_json::from_str(&text) {
                            handle_message(udp_msg, addr, &socket, &clients).await;
                        }
                    }
                    WsMessage::Binary(_) => {
                        // 处理二进制消息如果需要
                    }
                    WsMessage::Close(_) => {
                        break;
                    }
                    _ => {}
                }
            }
        }
        println!("WebSocket 客户端已断开");
    })
}
