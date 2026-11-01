# Friday 语音助手

- [x] 三种ssh类型的端口转发
  ![](./docs/Connections.png)

- [x] Friday语音助手，音频传过来之后自动播放，播放AI任务完成总结（配合pi agent使用）
  ![](./docs/main.png)

- [x] 本地音频录制：开始、暂停、继续、结束、回放、打开文件夹；全局 `F8` 开始/停止；WAV 按日期编号保存至程序旁的 `media/`
  - Windows/macOS 使用 CPAL，Linux 使用 `arecord`；macOS 首次录音需授权麦克风。
  - 工具栏下方显示接收状态、录音计时、实时音量波形及错误信息。

- [ ] Friday发送录音并下达tasks给AI
