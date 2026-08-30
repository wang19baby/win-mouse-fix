# Win Mouse Fix — 开发路线图

> 最后更新: 2026-08-30
> 当前重点: Phase 13（手机触控板）+ Phase 16（设备层深化）

---

## 已完成（Phase 0–10）

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

**138 测试通过，0 失败**

---

## Phase 13 — 手机触控板（当前重点）

### 13.1 基础连通性 ✅ 已完成
- [x] HTTP 服务器（嵌入式 HTML）
- [x] WebSocket 服务器（手写 RFC 6455）
- [x] Token 认证 + QR 码
- [x] 防火墙规则
- [x] 注入管线对接（send_mouse_move / push_remote_scroll / send_remote_click）
- [x] Anti-feedback-loop 守卫
- [x] 单元测试 + 集成测试框架

### 13.2 触控参数配置化
**目标**：将 JS 硬编码参数移入 `config.toml`，支持服务端调优

- [ ] 新增 `[touch]` 配置段到 `config.rs` + `config.toml`
  ```toml
  [touch]
  enabled = true
  gain = 6.7              # 基础增益（手指 px → 光标 px）
  accel_ref = 10           # 加速阈值（px/frame）
  accel_slope = 10         # 加速斜率
  accel_max_mult = 2.0     # 最大加速倍率
  move_ema = 0.35          # 速度 EMA 平滑系数
  scroll_gain = 5.5        # 滚动增益
  tap_ms = 220             # 点击最大时长
  tap_px = 10              # 点击最大位移
  swipe_px = 45            # 滑动触发距离
  ```
- [ ] 服务端读取 `[touch]` 配置，通过 WS status 下发给手机端
- [ ] 手机端 JS 从 status 消息读取参数（替代硬编码常量）
- [ ] GUI 添加"触控板"标签页（灵敏度/加速度滑块）

### 13.3 滚动平滑适配
**目标**：解决触摸连续 delta 输入到离散 wheel tick 平滑器的不匹配问题

- [ ] 分析问题：当前 `ScrollInjector` 针对 120-unit 离散 tick 调参，触摸连续小 delta 会"飘"
- [ ] 方案 A：触摸滚动绕过 `ScrollInjector`，直接 `SendInput` wheel（简单但无惯性）
- [ ] 方案 B：为触摸输入添加独立平滑参数（`touch_smooth_level` / `touch_smooth_trend`）
- [ ] 方案 C：在 `push_remote_scroll` 中做预处理（累积到 120-unit tick 再喂入）
- [ ] 实现选定方案，添加触摸专用配置项
- [ ] A/B 测试验证手感

### 13.4 手势体验优化
- [ ] 手势消歧逻辑优化（scroll vs pinch 的 `DECIDE_PX` 阈值）
- [ ] 3/4 指滑动灵敏度调整
- [ ] 对角线手势判定优化（`DIAG_RATIO` 参数）
- [ ] 手势视觉反馈增强（手机端 ripple + PC 端 HUD）
- [ ] 长按拖动 = 窗口拖拽（`FakeDrag` 模式）

### 13.5 PWA 完善
- [ ] 创建 `manifest.json`（名称、图标、主题色、display: standalone）
- [ ] 添加 Service Worker（离线缓存）
- [ ] iOS Safari "添加到主屏幕" 引导 UI
- [ ] 全屏模式：隐藏地址栏 + 状态栏
- [ ] 横屏锁定（可选）

### 13.6 状态下推
- [ ] 服务端 → 客户端 status 消息扩展：
  ```json
  {"t":"status","conn":true,"phone_battery":0.82,"locked":false,"pc_battery":54}
  ```
- [ ] 手机端 HUD 显示 PC 鼠标电量（从 `cache::BATTERY` 读取）
- [ ] 手机端显示手机电量（`navigator.getBattery()`）
- [ ] 锁屏状态下推（PC 端锁定 → 手机端禁用输入）

### 13.7 网络鲁棒性
- [ ] 多网卡 LAN IP 枚举（`GetAdaptersAddresses`，替代 UDP trick）
- [ ] 断线重连优化（指数退避 + 状态遮罩）
- [ ] WebSocket ping/pong 真机验证（15s+ 空闲场景）
- [ ] 连接质量指示（延迟/丢包率）

### 13.8 语音输入（后置）
- [ ] 协议扩展：`{"t":"key","text":"..."}` 消息类型
- [ ] 服务端 handler：逐字符 `SendInput` 键盘注入
- [ ] 手机端 UI：文本框 + 语音按钮
- [ ] IME 集成（系统输入法麦克风）

---

## Phase 16 — 设备层深化（当前重点）

### 16.1 G Hub 集成优化
**目标**：减少对 G Hub 的干扰，提升用户体验

- [ ] 静默启动优化：`lghub_system_tray.exe --minimized` 无弹窗
- [ ] 启动等待优化：检测 G Hub 就绪状态（端口 9010 可连接）而非固定 sleep
- [ ] G Hub 版本兼容性检测（读取注册表或进程版本）
- [ ] G Hub 未安装时的降级方案提示
- [ ] 设备列表缓存（减少 WebSocket 查询频率，5 分钟刷新）

### 16.2 按钮绑定读取
**目标**：读取 G502 LIGHTSPEED 的当前按钮绑定

- [ ] 探索 G Hub WebSocket 更多 API 端点
- [ ] 尝试 `POST` / `PUT` 方法（当前只用 `GET`）
- [ ] 尝试 WebSocket binary 帧的不同格式
- [ ] 分析 G Hub 源码/文档（如果有）
- [ ] 实现按钮绑定读取（通过 G Hub WS 或 HID++ 探索）
- [ ] 将按钮绑定信息暴露给 GUI（显示当前配置）

### 16.3 多设备支持
**目标**：同时管理多个罗技设备

- [ ] 设备列表 UI（显示所有检测到的设备）
- [ ] 按设备 ID 区分配置（`config.toml` 支持 `[device.xxx]`）
- [ ] 设备切换时自动加载对应配置
- [ ] 多设备电池状态显示（托盘图标切换）
- [ ] 设备热插拔检测（`WM_DEVICECHANGE`）

### 16.4 板载内存直接操作
**目标**：绕过 G Hub，直接读写设备配置

- [ ] 研究 HID++ 板载内存 feature（0x0001 或 0x0003）
- [ ] 实现板载 profile 读取
- [ ] 实现板载 profile 写入
- [ ] 板载 vs 软件模式切换
- [ ] 备份/恢复设备配置

### 16.5 按钮重映射集成
**目标**：将 G Hub 读取的按钮绑定与 win-mouse-fix 的 RemapEngine 对接

- [ ] 定义按钮 ID 映射（G1-G11 → 内部按钮码）
- [ ] 支持从 G Hub 读取的绑定作为默认值
- [ ] GUI 中显示当前按钮绑定
- [ ] 支持修改按钮绑定（通过 RemapEngine）
- [ ] 冲突检测（G Hub 绑定 vs win-mouse-fix 绑定）

---

## 任务依赖关系

```
Phase 13（手机触控板）
├── 13.2 触控参数配置化 ← 基础，其他任务依赖
├── 13.3 滚动平滑适配 ← 依赖 13.2
├── 13.4 手势优化 ← 依赖 13.2
├── 13.5 PWA 完善 ← 独立
├── 13.6 状态下推 ← 独立
├── 13.7 网络鲁棒性 ← 独立
└── 13.8 语音输入 ← 后置，依赖 13.6

Phase 16（设备层深化）
├── 16.1 G Hub 优化 ← 基础
├── 16.2 按钮读取 ← 依赖 16.1
├── 16.3 多设备 ← 独立
├── 16.4 板载内存 ← 独立
└── 16.5 按钮集成 ← 依赖 16.2 + 16.5
```

---

## 建议执行顺序

| 周 | 任务 | 交付物 |
|---|---|---|
| 本周 | 13.2 触控参数配置化 | `[touch]` 配置段 + WS 下发 |
| 本周 | 16.1 G Hub 优化 | 静默启动 + 就绪检测 |
| 下周 | 13.3 滚动平滑适配 | 触摸专用平滑参数 |
| 下周 | 16.2 按钮读取探索 | G Hub WS 按钮绑定 API |
| 2 周内 | 13.5 PWA 完善 | manifest.json + 全屏 |
| 2 周内 | 16.3 多设备支持 | 设备列表 UI |
| 1 个月内 | 13.4 + 13.6 | 手势优化 + 状态下推 |
| 1 个月内 | 16.4 + 16.5 | 板载内存 + 按钮集成 |

---

## 技术债务

| 项目 | 优先级 | 说明 |
|---|---|---|
| 清理编译警告 | 低 | 66 个 warnings |
| dpi-probe 构建错误 | 低 | 独立二进制 |
| 测试覆盖率提升 | 中 | 集成测试需交互式会话 |
| 性能基准测试 | 高 | 滚动延迟、CPU 占用基准 |
