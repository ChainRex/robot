use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use tokio::time::{self, Duration};

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

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // 绑定 UDP 服务器地址
    let socket = Arc::new(UdpSocket::bind("0.0.0.0:8080").await?);
    println!("服务器已启动，监听地址 0.0.0.0:8080");

    // 使用 Arc 和 Mutex 来共享客户端信息
    let clients = Arc::new(Mutex::new(ClientManager::new()));

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
    last_heartbeat: HashMap<(ClientType, String), u64>, // (类型, ID) -> 时间戳
}

impl ClientManager {
    fn new() -> Self {
        ClientManager {
            control_clients: HashMap::new(),
            robot_clients: HashMap::new(),
            last_heartbeat: HashMap::new(),
        }
    }

    fn register(&mut self, client_type: ClientType, client_id: String, addr: std::net::SocketAddr) {
        match client_type {
            ClientType::Control => {
                self.control_clients.insert(client_id.clone(), addr);
                println!("注册控制端: ID = {}, 地址 = {}", client_id, addr);
            }
            ClientType::Robot => {
                self.robot_clients.insert(client_id.clone(), addr);
                println!("注册机器人端: ID = {}, 地址 = {}", client_id, addr);
            }
        }
        // 更新心跳时间
        self.last_heartbeat
            .insert((client_type, client_id), current_timestamp());
    }

    fn update_heartbeat(&mut self, client_type: ClientType, client_id: String, timestamp: u64) {
        let client_id_clone = client_id.clone();
        self.last_heartbeat
            .insert((client_type, client_id), timestamp);
        println!(
            "收到心跳: 类型 = {:?}, ID = {}, 时间戳 = {}",
            client_type, client_id_clone, timestamp
        );
    }

    fn get_robot_clients(&self) -> Vec<std::net::SocketAddr> {
        self.robot_clients.values().cloned().collect()
    }

    fn get_control_clients(&self) -> Vec<std::net::SocketAddr> {
        self.control_clients.values().cloned().collect()
    }

    fn cleanup(&mut self) {
        let threshold = current_timestamp() - 60; // 60 秒超时
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
                        println!("控制端超时: ID = {}, 地址 = {}", client_id, addr);
                    }
                }
                ClientType::Robot => {
                    if let Some(addr) = self.robot_clients.remove(&client_id) {
                        println!("机器人端超时: ID = {}, 地址 = {}", client_id, addr);
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
    match msg {
        Message::Register(data) => {
            let client_type = data.client_type;
            let client_id = data.client_id;
            let mut clients = clients.lock().await;
            clients.register(client_type, client_id, addr);
        }
        Message::ControlCommand(data) => {
            println!(
                "收到控制指令: {:?}, 时间戳 = {}",
                data.command, data.timestamp
            );
            let serialized = serde_json::to_vec(&Message::ControlCommand(data)).unwrap();
            let clients = clients.lock().await;
            let robot_clients = clients.get_robot_clients();
            for robot_addr in robot_clients {
                if let Err(e) = socket.send_to(&serialized, robot_addr).await {
                    eprintln!("转发控制指令到 {} 失败: {}", robot_addr, e);
                } else {
                    println!("转发控制指令到 {}", robot_addr);
                }
            }
        }
        Message::ImageData(data) => {
            println!("收到图像数据, 时间戳 = {}", data.timestamp);
            let serialized = serde_json::to_vec(&Message::ImageData(data)).unwrap();
            let clients = clients.lock().await;
            let control_clients = clients.get_control_clients();
            for control_addr in control_clients {
                if let Err(e) = socket.send_to(&serialized, control_addr).await {
                    eprintln!("转发图像数据到 {} 失败: {}", control_addr, e);
                } else {
                    println!("转发图像数据到 {}", control_addr);
                }
            }
        }
        Message::ImageFragment(data) => {
            println!(
                "收到图像分片: {}/{}, 时间戳 = {}",
                data.sequence, data.total, data.timestamp
            );
            let serialized = serde_json::to_vec(&Message::ImageFragment(data)).unwrap();
            let clients = clients.lock().await;
            let control_clients = clients.get_control_clients();
            for control_addr in control_clients {
                if let Err(e) = socket.send_to(&serialized, control_addr).await {
                    eprintln!("转发图像分片到 {} 失败: {}", control_addr, e);
                } else {
                    println!("转发图像分片到 {}", control_addr);
                }
            }
        }
        Message::Heartbeat(data) => {
            let client_type = data.client_type;
            let client_id = data.client_id;
            let timestamp = data.timestamp;
            let mut clients = clients.lock().await;
            clients.update_heartbeat(client_type, client_id, timestamp);
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
