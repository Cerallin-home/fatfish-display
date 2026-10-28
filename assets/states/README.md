# states/

DSH 挂掉 / 恢复时播的两段，绿幕出片、抠成 RGBA，只叠人物不盖面板。

- `dsh_down.clip` — 50 帧 @30fps（1.67s），25MB，往左倒、带腿
- `dsh_up.clip` — `dsh_down.clip` 整段倒放（帧序反转后写死），50 帧 @30fps，25MB：
  首帧 = 倒下末帧，末帧 = 倒下首帧 = 立绘站姿，两头自动对上，不用另外生成
  - 尾帧轮廓比立绘小 6px（212 vs 218，源片首帧本来就这样），硬切基本看不出来
  - 生成专属起身片的尝试都放弃了（模型末帧大小不可控、撑地接缝对不齐），
    原片留在板子的 `~/fatfish/seedance/` 下，不进仓库
- 帧只留人物，alpha 由绿幕 key（chromakey + despill）出来
- 落点 (139,63) 按立绘在面板上的位置 (233,108) 反算：第一帧和站着的立绘重合，切换不跳
- 到头就停在最后一帧

转码（脚本在 `~/fatfish/seedance/`，原片也在那儿）：

    prep_clip_alpha.py dsh_down_green_4s.mp4 ../display-rs/assets/states/dsh_down.clip 30 50 0

触发：采样线程每秒 `systemctl --user is-active dsh-qqbot.service`，
不可用（连续 2 次）就播倒下，恢复可用就播起身。
开机时面板常比 dsh 先起来，所以启动会同步探一次：没在跑就直接躺着，
不播摔倒（那一刻 dsh 根本没启动过，不该有摔倒动作）。

预览：`FF_DOWN_AT=<秒> FF_UP_AT=<秒>` 到点切进倒下 / 起身，各触发一次；
`FF_DSH_UNIT=<别的 unit>` 换一个被盯的 unit，`FF_DSH_NEED=<次数>` 改去抖次数（默认 2）。
