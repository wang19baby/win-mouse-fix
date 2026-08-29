# Phase 11 — 手机应急妙控板（Web PWA 对接托盘）开发计划（修订版）

> 状态：**计划整顿**（经一轮攻击评审后修订）。未实现，待拍板进 PoC。
> 目标：手机浏览器扫码 → 打开全屏妙控板网页 → 经局域网驱动 Windows 光标/滚动/手势，
> 作为**真实鼠标损坏时的应急触控板**。手感复用现有 `SendInput` 注入管线，与真实鼠标一致。

---

## 0. 修订依据

上轮攻击点（🔴 阻断 / 🟡 需补 / 🟢 优化）逐条闭环，见 §3 闭环表。本版相对初版的关键修正：

1. **砍掉 SSE**，状态流改走 WebSocket 下行帧（A2：SSE 无 auth 头、双连接冗余）。
2. **补防火墙 + LAN IP 发现**（A1：不补则手机永远连不上）。
3. **删 `navigator.vibrate`**（A3：iOS Safari 不支持）。
4. **3 指导航走独立 TouchGestureRecognizer**，不复用 DragController（A4：DragController 由鼠标按键驱动，触摸无按键）。
5. **触摸专用增益/平滑曲线**，与鼠标解耦（B1）。
6. **多指消歧阈值 + iOS 触摸细节 + 手机电量语义 + QR 显示窗口 + 每连接线程 + 威胁模型**（B2–B7）。
7. **延迟定位 / ghost cursor / 重连退避 / A2HS 全屏**（C1–C4）。

---

## 1. 架构定稿

```
手机浏览器(全屏 PWA)
   │  Pointer Events: 单指移/点按/双指滚·捏合/3指导航
   │  rAF 合批 → JSON 帧
   ▼  WebSocket (ws://<LAN_IP>:<port>/ws, 首帧 token 认证)
托盘进程(常驻, 已有消息泵)
   │  监听线程 accept → 每连接 spawn 线程 → 解析 JSON
   ▼  按 type 分发:
       move   → injector::send_mouse_move(dx,dy)        [触摸专用增益]
       scroll → SCROLL_TX.send(WheelInput)             [触摸专用曲线]
       tap    → SendInput 左/右键
       gesture→ TouchGestureRecognizer → SendInput(Win+Tab / Ctrl+Win+←→)
       key    → SendInput 逐字符(语音后置)
   ▲  下行 status 帧(连接态 / 手机电量)回吐网页
网页 HUD: 连接点 + 手机电量 + 锁屏 + 灵敏度/自然滚动 + 断线遮罩
```

**已验证可复用（核对 `src/win/hooks.rs` / `src/scroll/injector.rs`）**
- `static SCROLL_TX: Mutex<Option<Sender<WheelInput>>>`：钩子→注入线程本就是 `mpsc` channel，WS 线程复用同模式，无需新造 IPC。
- `LLMHF_INJECTED` 守卫：自身 `SendInput` 注入事件被钩子忽略，**远程输入→SendInput→钩子不会死循环**。
- `pub fn send_mouse_move(dx,dy)`：可直接被 WS 线程调用发相对移动。

**已否决项**
- ❌ 原生安卓/iOS app：双端成本高、iOS 不能当 BT HID 外设。
- ❌ 蓝牙 HID：网页走 IP 传输，蓝牙在此无意义；且 iOS 不支持 HID Device 角色。
- ❌ SSE：WS 已双向，SSE 无 auth 头且冗余（A2）。

**语音（后置，P11.4）**：网页常驻文本框 → 调起系统输入法 → 输入法麦克风做语音→文本 → commit 后逐字 `key` 发 PC。本期仅留 UI 与接口。

---

## 2. 开发阶段

### P11.0 托盘本地服务（传输层）— 含 A1/A2/A5/B6/B7

- **LAN IP 发现**：`GetAdaptersAddresses` 枚举，取 `IfType` 为以太网/WiFi、`OperStatus=IfOperStatusUp`、且非 `127.*` / `169.254.*` / VPN / WSL / 虚拟网卡的活动 IPv4。多候选时取默认路由出口地址。
- **绑定具体 LAN IP**（非 `0.0.0.0`）：避免暴露到公网 WiFi（B7）。

- **防火墙（已实装）**：点击"手机妙控板"时若检测到入站规则 `WinMouseFix-Trackpad-<port>` 缺失，先弹 MessageBox 征求同意；同意则 `ShellExecuteW("runas")` 拉一次 UAC，经 `netsh advfirewall firewall add rule name=WinMouseFix-Trackpad-<port> dir=in action=allow protocol=TCP localport=<port>` 加**持久**入站规则（重启仍在）。用户拒绝则本次不配置，会话内不再追问。这是"连不上"的头号原因（参考 LapDeck）。
- **HTTP 路由**：`/` 返回网页静态资源（打包进 exe，`include_bytes!` 或 `resources`）；`/ws` 升级 WebSocket。

- **QR 显示（A5）**：托盘菜单"手机妙控板"→ `ShellExecute` 打开默认浏览器到 `/qr` 路由；该路由由服务端用 `qrcode` 算出矩阵、前端 canvas 绘制二维码 + 可复制地址 + "打开妙控板"链接。复用现有 Web 服务，无需 Win32 自绘窗口（后者在本机 `CreateWindowExW` 重入创建返回 NULL，已弃用）。
- **服务器模型（B6）**：手写监听线程 `accept` 后**每连接 `spawn` 线程**读取 WS 帧；单线程 `read` 会阻塞其他客户端。
- **token 配对（B7）**：启动时生成随机 256-bit token，写入 QR URL（`?t=`）。WS 首帧须 `{t:"auth",token}`，否则 3s 断。token 随 QR 轮换（删 `data/secret` 重启即换）。
- **依赖选型**：PoC 用**方案 (a) 手写极简**（单监听线程 + 手动 RFC6455 WS 握手 + 每连接线程），零新增 async 依赖，贴合项目风格。若后续加键盘/屏幕镜像再上 axum。

### P11.1 WS 输入/状态协议（JSON 信封，PoC）

客户端→服务端：
```json
{"t":"auth","token":"<256bit>"}
{"t":"move","dx":12,"dy":-3}
{"t":"scroll","dx":0,"dy":40}
{"t":"tap","b":"left"|"right"}
{"t":"gesture","g":"taskview"|"desk_l"|"desk_r"}
{"t":"key","text":"你好"}
```
服务端→客户端（替代 SSE 的状态流）：
```json
{"t":"status","conn":true,"phone_battery":0.82,"locked":false}
{"t":"reject"}
```
- 收包线程解析后分发：`move`/`scroll`→注入管线；`tap`/`gesture`→`SendInput`；`key`→键盘注入（后置）。
- **rAF 合批**：网页每帧最多发一次最新 `move`/`scroll` delta，避免事件洪泛。
- 二进制帧为后续优化（降延迟），本期 JSON 足够。

### P11.2 注入对接（复用现有管线，含 A4/B1）

- **move** → `injector::send_mouse_move(dx,dy)`。增益走**独立 `[touch]` 配置段**（B1）：手指 delta(扩展像素) 经 `gain × accel_curve` 映射到 PC 像素位移；与鼠标 `accel::PointerAccel` 参数解耦（手机屏小，增益高一个数量级）。
- **scroll** → `SCROLL_TX.send(WheelInput)`。平滑曲线走**触摸专用参数**（B1）：现有双指数 smoother 为离散 120 单位轮子 tick 调参，喂连续小 delta 会双平滑发飘；新增 touch 档（连续 delta、惯性衰减更短）。
- **tap** → `SendInput` `MOUSEEVENTF_LEFTDOWN/UP` 或 `RIGHTDOWN/UP`。
- **gesture（A4）** → 独立 `TouchGestureRecognizer`（不在 `gesture/` 的 DragController 内）：识别 3 指上滑→`Win+Tab`、3 指左右→`Ctrl+Win+←/→`（复用 P3 navigate 语义动作，但触发源是触摸而非鼠标按键）。
- **注入守卫已验证**：`LLMHF_INJECTED` 忽略自身注入，无回环。

### P11.3 妙控板网页（全屏 PWA，含 A3/B2/B3/B4/C2/C3/C4）

- **事件层**：Pointer Events，按 `pointerId` 追踪多指；`touch-action:none`；`user-select:none`；禁右键菜单。
- **iOS 细节（B3）**：`touchmove` 监听 `{passive:false}` + `preventDefault()`；`preventDefault` `gesturestart`/`gesturechange` 防橡皮筋/双击缩放；viewport `user-scalable=no, maximum-scale=1`。
- **手势表 + 消歧阈值（B2）**：
  | 手势 | 条件 | 动作 |
  |---|---|---|
  | 单指移动 | 持续 | `move` |
  | 单指点按 | 时长<200ms 且 位移<8px | 左键 `tap` |
  | 单指长按+移 | 时长>400ms 后移动 | 按住拖（`move` + 维持按下） |
  | 双指 tap | 两指短触 | 右键 `tap` |
  | 双指质心位移 | 间距变化率 < 捏合阈 | `scroll`(纵+横) |
  | 双指捏合 | 间距变化率 ≥ 阈 | 缩放(Ctrl+wheel，后置) |
  | 三指上滑 | 三指 + 上位移 | `gesture:taskview` |
  | 三指左右 | 三指 + 横向位移 | `gesture:desk_l/desk_r` |
  | 归属规则 | 双指中抬起 1 指 | 继续滚 / 取消（取继续滚，松开全部指才结束） |
- **HUD + 电量语义（B4）**：显示**手机电量**（`navigator.getBattery()` 上行，非 PC 鼠标电量——应急场景鼠标可能已坏/非罗技）+ 连接点。PC 鼠标电量仅作有则显示的附加。
- **锁屏**：锁后任何触控不生效（防放口袋误触），参考 deskpad。
- **设置（HUD 内）**：灵敏度滑块、自然滚动开关（反转）、加速度档——本地存，可经 `status` 下行与 `config.toml` 联动。
- **触觉（A3）**：**删除 `navigator.vibrate`**（iOS 不支持），改视觉涟漪反馈点击。
- **ghost cursor（C2）**：可选经 `status` 回显 PC 光标坐标，网页画半透明幽灵光标，缓解"看不到光标"的懵感（牺牲少量延迟）。
- **重连退避（C3）**：WS 断线指数退避重连，避免 tight loop 烧电；断线显示遮罩。
- **全屏（C4）**：`apple-mobile-web-app-capable` + manifest，`100dvh` 防地址栏抖动；引导"添加到主屏幕"才真全屏（iOS Safari 直接打开仍有地址栏）。

### P11.4 语音（后置）

- 网页常驻文本框，聚焦调起系统输入法；输入法麦克风做语音→文本；commit 后逐字 `key` 发 PC（复用键盘注入接口）。本期仅留 UI 与 `key` 协议字段，逻辑后置。

### P11.5 验收

- LAN 真机：单指移 / 点按左键 / 双指滚 / 右键 / 3 指任务视图·桌面切换。
- 体感延迟（C1）：2.4G WiFi 实测 15–40ms 抖动，定位"应急可用"，非日常主力。
- 锁屏生效；断线指数重连；防火墙放行后首次连接成功；多网卡取到正确 LAN IP。

---

## 3. 攻击点闭环表

| 攻击 | 等级 | 问题 | 本计划闭环位置 |
|---|---|---|---|

| A1 | 🟢 | 防火墙 + LAN IP 发现缺失 | P11.0：GetAdaptersAddresses 过滤 + 绑定具体 IP + 点击时弹确认经 UAC 放行 |
| A2 | 🔴 | SSE 冗余 + 无 auth 头 | 架构否决 SSE；状态走 WS 下行帧（P11.1） |
| A3 | 🔴 | iOS 不支持 `navigator.vibrate` | P11.3：删除，改视觉涟漪 |
| A4 | 🔴 | 复用 DragController 错配（按钮驱动 vs 触摸） | P11.2：独立 TouchGestureRecognizer |
| B1 | 🟡 | 触摸增益/曲线直接复用鼠标 | P11.2：`[touch]` 独立增益 + 触摸专用平滑曲线 |
| B2 | 🟡 | 多指消歧阈值未定义 | P11.3：手势表 + 明确阈值 + 归属规则 |
| B3 | 🟡 | iOS 触摸细节漏 | P11.3：passive:false + gesture 事件拦截 |
| B4 | 🟡 | 电量语义错位（应急=鼠标坏） | P11.3：HUD 显示手机电量，非 PC 鼠标电量 |

| B5 | 🟢 | 托盘菜单显示二维码 | P11.0：打开浏览器到 `/qr` 路由，前端 canvas 绘制二维码（服务端算矩阵） |
| B6 | 🟡 | 手写服务器单线程阻塞 | P11.0：accept 后每连接 spawn 线程 |
| B7 | 🟡 | 威胁模型含糊 | P11.0 + §4：绑具体 IP + auth 失败即断 + 仅信任 LAN |
| C1 | 🟢 | 延迟预期吹 | §4 定位：应急可用，非日常主力 |
| C2 | 🟢 | 看不到 PC 光标 | P11.3：ghost cursor（可选） |
| C3 | 🟢 | 重连 tight loop | P11.3：指数退避 |
| C4 | 🟢 | iOS 不全屏 | P11.3：A2HS + manifest + 100dvh |

---

## 4. 风险与约束（更新）

1. **延迟定位（C1）**：WiFi（尤 2.4G）实测 15–40ms 抖动。仅够应急，不是日常鼠标替代。文档写明定位。
2. **UIPI / 权限**：`SendInput` 注入受完整性级别限制（沿用 PLAN 风险）；托盘需足够权限运行 / 声明 manifest。
3. **防火墙 / IP（A1）**：不写规则手机连不上；多网卡取错 IP则扫码打不开。
4. **威胁模型（B7）**：仅信任家庭 LAN；绝不公网 WiFi 暴露；绝不端口转发；token 随 QR 轮换。
5. **iOS 全屏（C4）**：须引导"添加到主屏幕"，仅 Safari 打开仍有地址栏。
6. **跨线程**：WS 收包线程 → `SCROLL_TX`(Mutex 包裹的 mpsc，send 不阻塞) / `send_mouse_move`(直接 SendInput)；沿用现有无回环守卫。

---

## 5. 验收标准（可测）

- [ ] 防火墙放行后，同 WiFi 手机扫码首次连接成功（取到正确 LAN IP）。
- [ ] 单指移动光标；点按=左键；双指滑=滚动（带惯性）；双指 tap=右键。
- [ ] 3 指上滑=任务视图；3 指左右=切换虚拟桌面。
- [ ] 锁屏后触控全失效；断网指数重连；重连后恢复。
- [ ] iOS Safari：页面不自身滚动/缩放；加到主屏后全屏；点击有视觉涟漪（无 vibrate 报错）。
- [ ] token 错误→3s 拒连；公网 WiFi 下服务不暴露（绑具体 IP）。
- [ ] 远程输入与真实鼠标走同一 `SendInput` 管线，手感一致，无回环。


## 4. PoC 实装结果（2026-08-29）

**范围**:Phase 11 计划整顿后的最小可行闭环已实装并通过测试。

### 4.1 落地产物

- `src/remote.rs`(新增,~570 行):LAN 服务 + 手写 RFC6455 WS(握手/帧解析/掩码)+ SHA1/base64 接受 + token 认证 + 协议分发 + 内嵌网页(`include_str!` 打包);新增 `/qr` 路由,由服务端算 QR 矩阵、前端 `assets/qr.html` canvas 绘制。零新增 async 依赖。
- `src/win/hooks.rs`:新增 `push_remote_scroll` / `send_remote_click` / `send_remote_gesture`,全部走 `SendInput` 与本地钩子同管线。

- `src/win/tray.rs`:新增菜单项"手机妙控板"(ID 1008)→ `ShellExecute` 打开默认浏览器到 `/qr` 路由(服务端渲染二维码 + 可复制地址);保留剪贴板复制兜底,浏览器打不开时弹窗提示地址已复制。
- `assets/trackpad.html`(新增):全屏妙控板 PWA,Pointer Events + 多指状态机(单指移/点按左键、双指滚/右键、3 指任务视图/桌面切换)+ 锁屏 + 指数退避重连 + 视觉涟漪(iOS 不支持 vibrate)。
- `Cargo.toml`:新增 `qrcode`/`serde_json`,补 `Win32_NetworkManagement_IpHelper`(未实际移植枚举,见下)。

### 4.2 验证
- `cargo build` 通过;现有 99 项测试不破,**总计 133 项全绿**。
- 新增单测:RFC6455 `ws_accept` 向量(SHA1+base64 正确)、SHA1("abc") 向量、base64 向量。
- 新增集成单测(loopback,真实 TcpStream):WS 握手 101 → 正确 token 收 `status{conn:true}`;错误 token 收 `reject` 并断连;`GET /` 返回 200 + 内嵌妙控板 HTML。

### 4.3 与计划的偏差(诚实记录)
- **LAN IP 发现**:计划 A1 要求 `GetAdaptersAddresses` 多网卡枚举;PoC 暂用 **UDP 默认路由探测**(`connect("8.8.8.8:80")` 取本地出口 IP),对"手机可达网口"即正确,且零 windows-sys 类型移植风险。无公网时回退为空。枚举式多网卡精筛待后续补。
- **触摸专用增益/曲线**:`[touch]` 配置段、`TouchGestureRecognizer` 类未独立抽出;PoC 增益/滚动灵敏度以网页常量(`SENS`/`SCROLL_GAIN`)暴露,手势识别内联在网页 JS 状态机。桌面端 `move` 直接 `send_mouse_move(dx,dy)`(无额外增益),滚动经 `SCROLL_TX` 复用平滑器。后续可把灵敏度迁到 `config.toml` 的 `[touch]` 段。
- **手机电量 HUD / ghost cursor / 双指捏合缩放 / 语音**:按计划后置,本 PoC 未做(网页已留 UI 与 `key` 协议字段接口)。

- **`netsh` 防火墙**:点击"手机妙控板"时检测规则缺失→MessageBox 征求同意→`runas` 拉 UAC 加持久入站规则(`WinMouseFix-Trackpad-<port>`);拒绝则本次不配置。非致命。


### 4.4 待真机手动验收
- 同 WiFi 手机扫码连接;单指/双指/3 指手势手感;iOS Safari 加到主屏全屏、无自身滚/缩、点击涟漪无报错;锁屏/断网重连。

### 4.5 已闭环缺陷：控制型 WS 空闲断连（2026-08-29）

- **现象**：手机连上妙控板后,横幅常驻「连接已断开,正在重连…」,无法恢复「已连接」;HTTP(`/qr`)在手机上可达,`ws_accept` 经 RFC6455 向量测试正确,headless 重连测试全过 —— 看似「握手/防火墙/token 都对,却连不上」。
- **根因**：`handle_conn` 对 stream 设 `set_read_timeout(Some(10s))`,该超时同样作用于 `ws_loop`;`read_frame` 把**任何**读错误(含空闲超时)都 `return Ok(None)` 吞掉,`ws_loop` 据此误判为 EOF 关闭连接。手机连上后不操作妙控板(空闲 >10s),服务端读超时→误判断开→手机 `onclose` 重连→空闲再断,横幅常驻。HTTP 不受影响(页面加载 <10s);headless 重连测试因连上即发帧也未触发空闲,故漏检。
- **修复**（`src/remote.rs`）：
  - 新增 `recv_exact`：区分 `ErrorKind::TimedOut`(→ `Err`,探活)与 EOF/其他(→ `Ok(false)`,关闭);`read_frame` 仅对 EOF 返回 `Ok(None)`。
  - `ws_loop`:移除致命 30s 超时关闭,改 `set_read_timeout(30s)`;`Err(_)` 分支发 WebSocket **ping(0x9)** 探活,浏览器自动 **pong(0xA)**,连接保持;仅当探活写失败(手机脱离范围)才回收连接。零新增依赖/async。
- **验证**：`ws_idle_test.py` 连上后静默 40s(服务端 30s 发 ping,客户端自动 pong),连接存活(`IDLE KEEPALIVE OK`);`cargo test` 135 项全绿;`ws_reconnect_test.py` 重连回归 `SERVER ACCEPTS RECONNECT: OK`。
- **真机待确认**：手机静置 >15s 不操作时横幅应消失并保持「已连接」;操作/断网恢复重连正常。
