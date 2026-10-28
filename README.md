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

## 已知问题：尾巴摆动时的横向撕裂（未解决）

立绘右下角那条尾巴摆到幅度最大时，屏幕上会横着裂开一条缝，上下像错开一两帧。眨眼、口型都在脸上，脏行少且集中，看不出来；尾巴每帧都换一大片像素，所以只有它露馅。

判断的根因：fbtft 走 deferred IO，驱动按自己的节拍（delay = HZ/fps = 33ms）搬被改过的页，搬的过程中应用可能已经写进下一帧。应用和驱动之间没有垂直同步，屏幕上就可能采到「一半新一半旧」的一帧，缝落在尾巴上。SPI 已经开到 125MHz（`dtoverlay=mhs35:rotate=270,speed=125000000,txbuflen=-1`），再往上不是瓶颈。

试过、都只压住没治好：

- 帧率从 30.0 改成 1000/33 ≈ 30.3fps，正对驱动节拍（c522917）
- 主循环改绝对时刻排程，渲染抖动不再累积成周期漂移（同一提交）
- 脏行按 24 行容差并成 1~3 段连续写，把「驱动能抓到的半帧窗口」从几十段缩到末尾一段（同一提交）

结果：从「每摆一次都裂」变成「偶尔裂」，幅度最大那几帧还是有。别以为改完就没事了。

还没试的方向：

- 尾巴单独减幅或降帧，让一帧里的脏行落进一段之内——投手改画风，治标
- 尾巴的脏行先写、身体后写，把撕裂赶到不显眼的角落——同上，只是骗眼睛
- 换 DRM + `mipi-dbi-spi`，靠 page flip 拿真正的 vsync，彻底不走 fbtft 的 deferred IO
- 双缓冲 + 整帧一次刷：一帧 300KB，125MHz 下理论 20ms，塞得进 33ms，但 fbtft 本身吃不满带宽，得实测

## 许可

MIT
