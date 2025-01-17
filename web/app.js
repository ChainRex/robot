document.addEventListener('DOMContentLoaded', () => {
    const client = new UDPClient();
    
    // 获取DOM元素
    const connectBtn = document.getElementById('connect-btn');
    const connectionStatus = document.getElementById('connection-status');
    const serverIp = document.getElementById('server-ip');
    const clientId = document.getElementById('client-id');
    const controlBtns = document.querySelectorAll('.control-btn');
    const videoFeed = document.getElementById('video-feed');

    // 连接按钮点击事件
    connectBtn.addEventListener('click', async () => {
        if (!client.connected) {
            connectBtn.textContent = '断开';
            await client.connect(serverIp.value, "3000", clientId.value);
        } else {
            client.disconnect();
            connectBtn.textContent = '连接';
        }
    });

    // 控制按钮点击事件
    document.getElementById('forward-btn').addEventListener('click', () => {
        client.sendCommand('forward');
    });

    document.getElementById('backward-btn').addEventListener('click', () => {
        client.sendCommand('backward');
    });

    document.getElementById('left-btn').addEventListener('click', () => {
        client.sendCommand('left');
    });

    document.getElementById('right-btn').addEventListener('click', () => {
        client.sendCommand('right');
    });

    // 连接成功回调
    client.onConnect(() => {
        connectionStatus.textContent = '已连接';
        connectionStatus.classList.add('connected');
        connectionStatus.classList.remove('disconnected');
        controlBtns.forEach(btn => btn.disabled = false);
    });

    // 断开连接回调
    client.onDisconnect(() => {
        connectionStatus.textContent = '未连接';
        connectionStatus.classList.add('disconnected');
        connectionStatus.classList.remove('connected');
        connectBtn.textContent = '连接';
        controlBtns.forEach(btn => btn.disabled = true);
    });

    // 处理接收到的消息
    client.onMessage((message) => {
        if (message.type === 'image_data') {
            videoFeed.src = `data:image/jpeg;base64,${message.data.image}`;
        }
        else if (message.type === 'image_fragment') {
            // TODO: 处理图像分片
            console.log('收到图像分片:', message.data.sequence, '/', message.data.total);
        }
    });

    // 添加键盘控制支持
    document.addEventListener('keydown', (event) => {
        if (!client.connected) return;
        
        switch(event.key) {
            case 'ArrowUp':
                client.sendCommand('forward');
                break;
            case 'ArrowDown':
                client.sendCommand('backward');
                break;
            case 'ArrowLeft':
                client.sendCommand('left');
                break;
            case 'ArrowRight':
                client.sendCommand('right');
                break;
        }
    });
}); 