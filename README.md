# fatfish-display

树莓派 5 + MHS-3.5 SPI 小屏（480×320）上的桌面挂件：左边是系统面板，右边是立绘。

- 面板：CPU / 内存 / 温度 / 负载四条进度条；服务只盯 `dsh` 和 `glances`；网络按物理网口逐行列出有线与 WiFi 的 IPv4，最后一段高亮，断开的网口标红「未连接」
- 立绘：30fps 循环动画（眨眼、口型），说到话时头顶上方弹对话气泡
- 后端：直接 mmap `/dev/fb0`，按脏行写 RGB565；不做 X / Wayland 依赖，开机 systemd user 服务拉起来就能显示
- 说话接口：监听 `$XDG_RUNTIME_DIR/fatfish-display.sock`（可用 `FF_SAY_SOCK` 覆盖），收到一段文本就换气泡；配合 [dsh-display](https://github.com/Cerallin-home/dsh-display) 让 agent 传话

## 跑起来

```bash
cargo build --release --offline
./target/release/fatfish-display fb 0 30        # fb 后端，跑满 30fps
FF_SAY="开机第一句" ./target/release/fatfish-display fb 0 30
echo '这是从外面传进来的话' | socat - UNIX-CONNECT:$XDG_RUNTIME_DIR/fatfish-display.sock
```

装成服务（`~/.config/systemd/user/fatfish-display.service`）：

```ini
[Service]
ExecStartPre=/bin/bash -c 'for i in $(seq 1 30); do [ -e /dev/fb0 ] && exit 0; sleep 1; done; exit 1'
ExecStart=/home/julia/fatfish/display-rs/target/release/fatfish-display fb 0 30
Restart=on-failure
```

## 面板朝向

面板本身歪着装的时候改 overlay，不要在软件里翻：

```
# /boot/firmware/config.txt
dtoverlay=mhs35:rotate=270
```

## 结构

```
src/model/      sampler：每秒采一次 /proc、/sys、systemctl、ip
src/viewmodel/  把事实翻成画面量（进度条比例、气泡状态、动画帧）
src/view/       canvas / text / sprites / panel：只吃 Snapshot 画像素
src/view/       mod.rs：fbdev 后端，mmap + 脏行提交
```

## 许可

MIT
