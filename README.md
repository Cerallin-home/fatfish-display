# fatfish-display

树莓派 5 + MHS-3.5 SPI 小屏（480×320）上的桌面挂件：左边是系统面板，右边是立绘。

- 面板：CPU / 内存 / 温度 / 负载四条进度条；服务只盯 `dsh` 和 `glances`；网络按物理网口逐行列出有线与 WiFi 的 IPv4，最后一段高亮，断开的网口标红「未连接」
- 立绘：30fps 循环动画（眨眼、口型），说到话时头顶上方弹对话气泡
- 后端：直接 mmap `/dev/fb0`，按脏行写 RGB565；不做 X / Wayland 依赖，开机 systemd user 服务拉起来就能显示
- 说话接口：监听 `$XDG_RUNTIME_DIR/fatfish-display.sock`（可用 `FF_SAY_SOCK` 覆盖），收到一段文本就换气泡；配合 [dsh-display](https://github.com/Cerallin-home/dsh-display) 让 agent 传话

## 指标配色

四条进度条常态蓝（`#4A9EFF`），过警戒线转黄（`#FFC840`），过危险线转红（`#FF524A`）：

| 指标 | 黄（警戒） | 红（危险） |
| --- | --- | --- |
| CPU | 70% | 90% |
| 内存 | 75% | 90% |
| 温度 | 60℃ | 70℃ |
| 负载（每核） | 0.7 | 1.0 |

负载用 load1 / 核心数折算成每核一条，和其他三条同一把尺子。温度到 60℃ 会同时触发立绘上方的告警气泡。

阈值全是 `src/viewmodel/mod.rs` 顶上的常量（`CPU_WARN` / `CPU_CRIT` / `MEM_WARN` / `MEM_CRIT` / `TEMP_WARN` / `TEMP_CRIT` / `LOAD_WARN` / `LOAD_CRIT`），改完 `cargo build --release --offline` 重启服务即可。

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
ExecStart=/home/julia/fatfish/fatfish-display/target/release/fatfish-display fb 0 30
Restart=on-failure
```

## 熄屏

晚上不想屏幕亮着：

```bash
./target/release/fatfish-display ctl off     # 熄屏
./target/release/fatfish-display ctl on      # 亮屏
./target/release/fatfish-display ctl status
```

- 开关走独立 socket（`$XDG_RUNTIME_DIR/fatfish-display.ctl`，`FF_CTL_SOCK` 可覆盖），不和气泡口混用
- 熄屏 = 整屏压成黑 + 停止刷新（帧率降到 5fps）。**不是关背光**：这块面板的背光直连 3.3V，overlay 里只有 reset / dc 两个 gpio，fbtft 也没实现 FBIOBLANK，软件关不掉
- **实测：黑屏之后背光还是亮的**，整个面板是一片均匀的灰白，房间里暗下来看很明显——这是「画面变黑」，不是「屏幕熄灭」。想要真关灯，只有两条路：给屏断电，或者把背光那一路飞线到 GPIO（见上面说的 3V3 + MOS）
- 熄屏期间送进来的气泡一律丢掉，屏幕不会因为一句话自己亮起来
- 状态落在 `$XDG_RUNTIME_DIR/fatfish-display.power`（`on` / `off`），脚本直接读这个文件，不用问进程

### 夜间自动熄屏

默认每天 **23:40 - 次日 08:00** 自动黑屏，出窗口自己亮回来。窗口写在 `src/main.rs` 的 `DEFAULT_SLEEP_WINDOW`，也能用环境变量覆盖：

```bash
FF_SLEEP=22:30-07:00 ./target/release/fatfish-display fb 0 30.3
FF_SLEEP=off ./target/release/fatfish-display fb 0 30.3   # 关掉自动
```

- 进窗口强制熄，出窗口强制亮；白天手动熄的不受影响（照旧黑着，不会被硬点亮）
- 夜里想看一眼：`ctl on` 给 10 分钟，到点自己黑回去（`NIGHT_WAKE_SECS`）
- 窗口每秒对一次表，用 `localtime_r`，不 fork `date`

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
