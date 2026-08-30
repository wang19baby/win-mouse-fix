# Win Mouse Fix — 开发路线图

> 最后更新: 2026-08-30
> 当前重点: Phase 13（手机触控板）+ Phase 16（设备层深化）

---

## 已完成（Phase 0–10 + 13–15 + 16.1）

| Phase | 功能 | 状态 |
|---|---|---|
| P0 | 基础验证 | ✅ |
| P1 | 平滑滚动 | ✅ 99 测试 |
| P2 | 按键重映射 | ✅ |
| P3 | 点击拖拽手势 | ✅ |
| P4 | 修改键组合 | ✅ |
| P5 | 指针加速 | ✅ |
| P6 | 配置 GUI + 热重载 | ✅ |
| P7 | 每应用配置文件 | ✅ |
| P8 | 设备层（电池+DPI+枚举） | ✅ |
| P9 | 电池托盘图标 | ✅ |
| P10 | DPI 跨屏自动切换 | ✅ |
| P13.1 | 手机触控板基础连通 | ✅ HTTP + WS + 注入管线 |
| P13.2 | 触控参数配置化 | ✅ `[touch]` 配置段 + WS 下发 + JS 读取 |
| P13.3 | 滚动平滑适配 | ✅ 触摸绕过平滑器，直接 SendInput |
| P13.5 | PWA 完善 | ✅ manifest.json + /manifest.json 路由 |
| P13.8 | 语音输入 | ✅ Web Speech API + KEYEVENTF_UNICODE 注入 |
| P14.1 | 退出确认对话框 | ✅ MessageBoxW YES/NO |
| P14.2 | 帮助文档 | ✅ 托盘菜单"帮助" |
| P14.3 | 错误上报 | ✅ panic hook → crash.log |
| P15 | 日志优化 | ✅ 可读时间戳 + 文件轮转(2MB×3) |
| P16.1 | G Hub 优化 | ✅ 端口轮询替代固定 sleep + 自动启动 |
| perf | 性能优化 | ✅ 热路径移除 log/锁/marker 修复 |
| debt | 编译警告清理 | ✅ 70 warnings → 0 |

**144 测试通过，1 flaky（timing），0 失败**

---

## Phase 13 — 手机触控板（进行中）

### 13.1 基础连通性 ✅ 已完成
- [x] HTTP 服务器（嵌入式 HTML）
- [x] WebSocket 服务器（手写 RFC 6455）
- [x] Token 认证 + QR 码
- [x] 防火墙规则
- [x] 注入管线对接（send_mouse_move / push_remote_scroll / send_remote_click）
- [x] Anti-feedback-loop 守卫
- [x] 单元测试 + 集成测试框架

### 13.2 触控参数配置化 ✅ 已完成
- [x] 新增 `[touch]` 配置段到 `config.rs` + `config.toml`
- [x] 服务端读取 `[touch]` 配置，通过 WS status 下发给手机端
- [x] 手机端 JS 从 status 消息读取参数（替代硬编码常量）

### 13.3 滚动平滑适配 ✅ 已完成
- [x] 触摸滚动绕过 `ScrollInjector`，直接 `SendInput` wheel

### 13.4 手势体验优化
- [ ] 手势消歧逻辑优化（scroll vs pinch 的 `DECIDE_PX` 阈值）
- [ ] 3/4 指滑动灵敏度调整
- [ ] 对角线手势判定优化（`DIAG_RATIO` 参数）
- [ ] 手势视觉反馈增强（手机端 ripple + PC 端 HUD）
- [ ] 长按拖动 = 窗口拖拽（`FakeDrag` 模式）

### 13.5 PWA 完善 ✅ 已完成
- [x] 创建 `manifest.json`（名称、图标、主题色、display: standalone）
- [ ] 添加 Service Worker（离线缓存）
- [ ] iOS Safari "添加到主屏幕" 引导 UI
- [ ] 全屏模式：隐藏地址栏 + 状态栏
- [ ] 横屏锁定（可选）

### 13.6 状态下推
- [ ] 服务端 → 客户端 status 消息扩展（PC 电量、手机电量）
- [ ] 手机端 HUD 显示 PC 鼠标电量（从 `cache::BATTERY` 读取）
- [ ] 手机端显示手机电量（`navigator.getBattery()`）

### 13.7 网络鲁棒性
- [ ] 多网卡 LAN IP 枚举（`GetAdaptersAddresses`）
- [ ] 断线重连优化（指数退避 + 状态遮罩）
- [ ] WebSocket ping/pong 真机验证
- [ ] 连接质量指示（延迟/丢包率）

### 13.8 语音输入 ✅ 已完成
- [x] 协议扩展：`{"t":"key","text":"..."}` 消息类型
- [x] 服务端 handler：`send_remote_text()` 逐字符键盘注入
- [x] 手机端 UI：🎤 按钮 + Web Speech API + 输入框
- [x] 语音优先：打开输入栏自动启动语音识别

---

## Phase 16 — 设备层深化

### 16.1 G Hub 集成优化 ✅ 已完成
- [x] 启动等待优化：端口轮询（200ms 间隔，15s 超时）替代固定 sleep
- [x] `ensure_lghub_running()`: 检查端口 → 检查进程 → 启动 → 等待就绪
- [x] 设备列表缓存（减少 WebSocket 查询频率）

### 16.2 按钮绑定读取 ❌ 阻塞
G Hub WebSocket `/card/{id}` 返回 `NO_SUCH_PATH`。可能路径：
- HID++ onboard memory（需进一步研究）
- 接受限制，不读取按钮绑定

### 16.3 多设备支持
- [ ] 设备列表 UI（显示所有检测到的设备）
- [ ] 按设备 ID 区分配置
- [ ] 设备热插拔检测（`WM_DEVICECHANGE`）

### 16.4 板载内存直接操作
- [ ] 研究 HID++ 板载内存 feature
- [ ] 实现板载 profile 读写
- [ ] 备份/恢复设备配置

### 16.5 按钮重映射集成
- [ ] 定义按钮 ID 映射（G1-G11 → 内部按钮码）
- [ ] GUI 中显示当前按钮绑定
- [ ] 支持修改按钮绑定（通过 RemapEngine）

---

## 建议执行顺序

| 优先级 | 任务 | 说明 |
|---|---|---|
| 高 | 13.4 手势消歧优化 | 提升触控板核心体验 |
| 高 | 13.6 状态下推 | 手机显示 PC 电量 |
| 中 | 13.7 网络鲁棒性 | 多网卡 + 断线重连 |
| 中 | 13.5 PWA Service Worker | 离线缓存 |
| 低 | 16.3 多设备支持 | 需多设备硬件 |
| 低 | 16.4 板载内存 | HID++ 研究 |
| 低 | 16.5 按钮集成 | 依赖 16.2 |

---

## 技术债务

| 项目 | 优先级 | 状态 |
|---|---|---|
| 编译警告清理 | 低 | ✅ 70 → 0 warnings |
| dpi-probe 构建错误 | 低 | 独立二进制，非关键 |
| 性能基准测试 | 中 | 滚动延迟、CPU 占用基准 |
| 测试覆盖率提升 | 中 | 集成测试需交互式会话 |
