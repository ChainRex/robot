class UDPClient {
    constructor() {
        this.socket = null;
        this.connected = false;
        this.heartbeatInterval = null;
        this.onMessageCallback = null;
        this.onConnectCallback = null;
        this.onDisconnectCallback = null;
    }

    async connect(serverIp, serverPort, clientId) {
        try {
            // 创建UDP socket
            this.socket = new WebSocket(`ws://${serverIp}:3000/ws`);
            this.clientId = clientId;

            this.socket.onopen = () => {
                this.connected = true;
                this.startHeartbeat();
                this.register();
                if (this.onConnectCallback) {
                    this.onConnectCallback();
                }
            };

            this.socket.onmessage = (event) => {
                if (this.onMessageCallback) {
                    this.onMessageCallback(JSON.parse(event.data));
                }
            };

            this.socket.onclose = () => {
                this.disconnect();
            };

        } catch (error) {
            console.error('连接失败:', error);
            this.disconnect();
        }
    }

    disconnect() {
        if (this.heartbeatInterval) {
            clearInterval(this.heartbeatInterval);
            this.heartbeatInterval = null;
        }
        
        if (this.socket) {
            this.socket.close();
            this.socket = null;
        }

        this.connected = false;
        
        if (this.onDisconnectCallback) {
            this.onDisconnectCallback();
        }
    }

    register() {
        const message = {
            type: 'register',
            data: {
                client_type: 'control',
                client_id: this.clientId
            }
        };
        this.send(message);
    }

    startHeartbeat() {
        this.heartbeatInterval = setInterval(() => {
            const message = {
                type: 'heartbeat',
                data: {
                    client_type: 'control',
                    client_id: this.clientId,
                    timestamp: Math.floor(Date.now() / 1000)
                }
            };
            this.send(message);
        }, 5000); // 每5秒发送一次心跳
    }

    sendCommand(command) {
        const message = {
            type: 'control_command',
            data: {
                command: command,
                timestamp: Math.floor(Date.now() / 1000)
            }
        };
        this.send(message);
    }

    send(message) {
        if (this.connected && this.socket) {
            this.socket.send(JSON.stringify(message));
        }
    }

    onMessage(callback) {
        this.onMessageCallback = callback;
    }

    onConnect(callback) {
        this.onConnectCallback = callback;
    }

    onDisconnect(callback) {
        this.onDisconnectCallback = callback;
    }
} 