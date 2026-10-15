# 项目收敛验证证据

原软件收敛记录验证日期：2026-10-19；当前工作树的远程拖选追加验收见文末。规格：`docs/project-completion-spec.md`；任务：`TODO.md`；总体开发顺序：`PLAN.md`。

## 结论与范围

本轮软件接入、质量门禁与受控运行验证完成。项目仍是 Windows 原生托盘 + 全局输入钩子 + Rust 工作线程架构，不是完整复刻 macOS 触控板协议。实际鼠标手感、Logitech 硬件写入、真实手机安装/多指输入及 UIPI/LAN 兼容性未验收，不能宣称产品全功能完成。

- 修改现有模块，未新增运行时依赖。用户现有 `config.toml` 选项未重置；已有焦点居中/PoC 保留，仅处理门禁相关测试辅助代码与格式。
- 设置/应用配置采用基础配置与窗口草稿，保存不扁平化 profile；非法或无法读取的文件不以运行时缓存覆盖。
- 指针加速原生透传，容量为 1 的队列只发送额外增量；屏幕坐标非 Raw Input。禁用使 generation 失效，不在钩子所属线程 join SendInput 工作者。
- 手机 SW 仅缓存静态 shell 和 manifest。`remote.local_only=true` 仅 loopback，跳过防火墙，供本地浏览器及独立 E2E 使用。
- 电量托盘只读 G Hub 后台缓存；自动硬件 DPI 定时器关闭，写入函数需读回一致才报告确认成功。
- 受控故障/注入探针已移除；临时 exe、配置、token 随自有实例清理；没有提交或回滚用户文件。

## 原软件收敛门禁（远程拖选追加前）

同一工作树依次执行，全部返回成功：

| 命令 | 观察结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo check --offline --all-targets` | 通过 |
| `cargo clippy --offline --all-targets -- -D warnings` | 通过 |
| `cargo test --offline --all-targets` | 259 passed，5 ignored，0 failed |
| `cargo build --offline --bin win-mouse-fix` | 通过，实际运行 debug exe |
| `npx --no-install playwright test` | 4 passed |

五个 ignored 测试是原有 loopback/真实 Win32 全局输入集成测试，本轮没有执行它们；不能计入已通过。手工 HTML 声明/括号/DOM 字符串检查已删除，由真正 Chromium 页面的认证、错误、导航与离线场景替代；本地配置选择断言已删除，未强改用户配置以使测试通过。Node 的环境颜色提示不影响退出状态。

## 实际运行证明

所有应用场景使用临时目录中的真实 exe 和独立配置，不复用正在运行的用户实例。

### Win32 GUI 与托盘

- 实际托盘弹出菜单读取到“设置...”“应用配置...”“打开配置目录”。
- 设置加载基础滚动速度 1.25，保存为 1.5 后 profile 中速度 2.0 保持不变；NaN 被错误对话框拒绝，文件字节未变。
- 设置 Cancel 丢弃 3.5 的未保存编辑；应用配置 Add 后 Cancel 不写盘，Add 后 OK 才持久化。
- “保存并录制”先保存速度 1.8 和已暂存的映射删除，再打开录制提示；未发送物理录制触发器。
- 已打开窗口后将独立配置改为非法 TOML，再保存会拒绝，非法文件未被缓存/默认配置覆盖。
- GUI 不再由不可见 `HWND_MESSAGE` 窗口拥有；DWM cloaking 为 0，窗口关闭不会退出主进程。调整高度，使操作按钮矩形位于客户区内。
- 原生像素观察使用自有设置窗口的 `PrintWindow`，可见滚动页与加载值；当前桌面对部分子控件的截图绘制不稳定，OK/Cancel 另用真实控件消息、位置、可见性和保存/取消行为验证，不宣称像素级回归验收。
- Win32 GUI/托盘受控场景通过托盘窗口 WM_CLOSE 正常退出，退出码 0；E2E 使用自有子进程生命周期清理。

### 安装失败恢复与指针工作者

- 临时探针仅让第二次钩子安装受控失败。修改配置后发生一次失败，未再改配置，下一次托盘定时器恢复安装；进程持续存活，退出码 0。探针随后删除。
- 临时 native 样本触发真实异步 SendInput；观察到一次工作者发送和一次注入事件重同步，没有额外发送回环。配置禁用后再给样本，不产生工作者发送；退出码 0，指针恢复原位。探针随后删除。
- 首次以光标坐标稳定性作断言的检查受会话光标变动影响失败，不能视为真实鼠标稳定性通过；后续以工作者发送/注入分支计数完成受控链路证明。未替代实体鼠标的高频、边缘和 UIPI 验收。

### 手机浏览器

`tests/e2e/fixture.js` 复制 debug exe 到临时目录，使用动态 loopback 端口与程序生成的 `v2:` token；不写凭据到仓库、不修改现有配置，不开启防火墙，不按镜像名终止进程。

四个真实 Chromium 场景：

1. `/sw.js` 正确 JavaScript MIME；安装并取得实际 SW controller；缓存只含三个 shell 与 manifest，键不带查询；QR 请求不进入缓存，无关应用缓存保留。断网后主页面及两个独立页面重新加载，主页面显示断线，不显示已连接。
2. 有效 token 的实际手机页面进入“已连接”，无页面脚本错误。
3. 无效 token 保持“认证失败”提示，不被 WS close 覆盖成假重连。
4. 手机页面打开独立窗口列表，再返回触控板；不点击窗口卡片，不注入桌面鼠标或键盘操作。

另通过浏览器工具打开真实页面，观察触控表面和已连接 HUD；自有 tab 与应用随后关闭。普通 LAN HTTP 不满足浏览器安全上下文，离线/安装能力不因此向手机开放。

### 设备安全边界

- 本机 G Hub 电量端点 127.0.0.1:9010 不可达；应用保持运行跨过 5 秒电量刷新，硬件 DPI checkbox 处于禁用状态。
- PnP 只提供通用 HID 鼠标名称，未取得可用于 Logitech 电量/HID++ 写后读回验收的设备证据；未执行硬件 DPI 写入。
- 真机电量、混合 DPI 跨屏、拔插、设备能力范围与通信超时仍列在 TODO，未经验证不启用自动写入。

## 审查与剩余验收

实施前读取指针路径的 bounded 架构意见；完成后做高风险代码审查。发现并修复：同配置不重试失败安装、前台 profile key 过早提交、录制丢失未保存草稿、已保存/未启用错误混淆。软件证据不构成独立形式化高保证，也不替代目标环境验收。

剩余顺序：实体鼠标输入手感和混合显示器 GUI → Logitech/G Hub 查询 → HID++ 写后读回与超时/热插拔 → 可信 HTTPS 下真实 iOS/Android → UIPI、真实 LAN/VPN/防火墙。详见 `TODO.md` 的 P2。

## 远程拖选收尾：合并前本地工作树（含未发布 PoC）

本节取代上方旧测试数量，补齐规格 R1–R12 的软件/受控运行证据，不替代 P2 真机验收。继续使用 `feat/project-completion-20261014` 及原工作树，未提交、未回滚用户改动，无新增依赖或软件安装。

- 规格：`D:\work_space\personal_workspace\win-mouse-fix\docs\project-completion-spec.md`。
- spec approval：not obtained（自主续作）；independent verification：not performed。不宣称完整 Tier 3 高保证。
- 最终入口：`powershell -NoProfile -ExecutionPolicy Bypass -File tools/drag-gauntlet.ps1`，退出码 0；所有下列数字来自最后一次完整执行，之后仅更新文档。
- 工具：rustc 1.96.0、Node v23.11.1、Python 3.13.1、Playwright 1.62.1（npm lockfile）、cargo-llvm-cov 0.9.1。
- 源码/验收输入 SHA-256：`ac24ef0655232653ca6bcda11e838d3492a40ac2aa83de07d80b0a44aa19b625`。复算：`python tools/drag_smoke.py --source-state`。摘要范围由该函数固定：Rust、选定 assets、测试/工具源码、Cargo/npm manifest/lockfile、资源与验证配置；不包含用户运行配置、token、文档或构建产物，不代表已提交或洁净工作树。

### 最终门禁与实际运行

| 层 / 命令 | 观察结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo check --offline --all-targets` | 通过 |
| `cargo clippy --offline --all-targets -- -D warnings` | 通过 |
| `cargo test --offline --all-targets` | 270 passed，5 ignored，0 failed |
| `cargo build --offline --bin win-mouse-fix` | 通过 |
| `npx --no-install playwright test` | 30 passed；其中两个真实触控页面共 26 个拖选场景 |
| `python tools/drag_smoke.py` | 自有 Win32 EDIT 中选中整句 `Remote drag selects this entire sentence.`，观察一次 down/up；重复/旧 id 与外来会话未抢占 |
| 原生租期 / TCP 断线 / 半个 WebSocket 帧 | 分别在 2.058s / 0.065s / 2.054s 观察到 up；不是仅检查协议回执 |
| 原生服务停止与重启 / 进程退出 | 停止释放；重启新会话可重新按下；持有左键时 WM_CLOSE 退出，观察到 up、系统左键状态清零、退出码 0 |
| `python tools/drag_mutation.py` | 3/3 实际错误被行为断言击杀；临时源码副本的基线与恢复后 11 个远程输入测试通过 |
| `cargo llvm-cov test --offline --bin win-mouse-fix --json --output-path target/drag-coverage.json remote_input::tests::` | 11 passed；`remote_input.rs` 行报告 541/718（75.35%，含测试代码）；仅诊断，不是变更行覆盖率门禁 |

五个原有 ignored 用例未执行，不能计为通过。原生脚本只复制并启动自己的 exe，在临时目录生成配置/token，用 loopback 跳过防火墙；仅操作自有文本窗和按 PID/类名确认的托盘窗，完成后恢复光标/前台窗口并清理实例。浏览器拖选用例拦截 OS 输入边界以避免操作用户桌面，因此不能单独证明 SendInput 或真实选择；上面的原生实测承担该证明。另用 browser 工具实际打开独立服务页面，观察到“已连接”和触控表面，截图后关闭自有 tab 与进程。

### 规格 → 证据与边界

| 规格 | 可执行证据 | 状态 |
|---|---|---|
| R1 正常拖选 | 原生整句选择与 down/up；浏览器 normal-up 用例验证带 id 的末帧在 end 前发送 | pass（受控桌面） |
| R2 重复/旧 id | 原生重复请求及 stale_id；`duplicate_and_retired_ids_cannot_press_twice` | pass |
| R3 会话/id 隔离 | 原生 foreign 请求；`foreign_and_stale_commands_cannot_touch_the_owner` 包含 foreign start 尚在排队的 live 状态 | pass |
| R4 物理按下/注入失败 | `physical_or_failed_down_never_creates_release_debt`，假注入器边界，不会补发 up | pass（状态机）；真实并发物理输入未验收 |
| R5 取消路径 | 两页面 pointercancel/lostcapture/secondfinger/blur/pagehide/hidden/WS close；无额外 tap、待发移动丢弃、续期停止 | pass（Chromium） |
| R6 独立回收 | 原生租期、真实断线与半帧；`heartbeat_keeps_stationary_drag_but_late_renew_does_not_resurrect_it` | pass（loopback） |
| R7 队列饱和/迟到 start | `saturated_queue_cannot_drop_end_or_cancel`、`cancelled_and_expired_queued_starts_never_inject_down` | pass（有界队列/状态机） |
| R8 释放/移动失败 | `failed_up_is_retained_until_success_and_cannot_be_renewed_away`、`move_failure_and_partial_tap_retain_release_debt`；浏览器 release-failed 用例 | pass（受控失败）；真实 UIPI 拒绝未验收 |
| R9 服务代际及正在 down 时取消 | 原生停止/重启；`cancellation_during_down_and_generation_stop_order_release_after_down` | pass（受控运行/状态机） |
| R10 正常退出 | 原生按下后 WM_CLOSE，实际 up 与退出码 0 | pass（受控桌面） |
| R11 两页面与 SW | 26 个拖选用例及离线 shell 用例（包含 `/drag.js`）；实际浏览器观察 | pass（Chromium） |
| R12 不变量 | 全量门禁、临时实例、本地配置/防火墙隔离；旧 longpress 按下/释放入口定向检索无匹配 | pass（所列检查），非穷尽证明 |

### 失败记录与保证降级

- 恢复的旧浏览器失败是 CDP 在 touchCancel 后重复发送 touchEnd；当前已有条件分支避免无效第二次结束，本次针对性 26 项和完整 30 项通过。
- 原生工具初稿缺必填配置、窗口被遮挡、Windows EPP 导致固定移动距离不稳定、EDIT 将快速第二次 down 改写为双击消息；分别修正独立配置、目标命中检查/临时顶层、反馈式移动和 down/双击观测。不放宽整句选择或 down/up 断言。初稿退出未匹配准确托盘类名，曾遗留一个自有实例；按其精确 exe 路径核验后关闭，临时目录已清理。
- 首次移除跨会话所有权检查的变异存活：旧断言只观察已被拒绝的非 live 会话。补上“foreign start 仍在排队且 live”的断言后，同一变异被捕获；另外两个变异分别是允许旧 id 复用、释放失败后丢债务。三者均实际编译运行并失败于指定行为测试，编译错误不算击杀。
- OpenCode 免费只读委派两次返回 `Unexpected server error`，未提供结果，未使用付费模型；最终判断/运行验证均在主会话完成，不能算独立审查。
- 100% 变更行/分支覆盖门禁、随机顺序全套运行、独立高风险审查未执行；覆盖率仅有上表的单测诊断。序列不变量使用已有确定性 `7^4 = 2401` 组输入，不等于线程调度穷举。未增加依赖，未做新的供应链扫描。
- 实体鼠标、目标 Logitech/G Hub/HID++、混合 DPI、真实 iOS/Android、提权 UIPI 与真实 LAN/VPN/防火墙仍待目标环境。安全桌面持续拒绝释放时退出可能等待；强杀/系统崩溃及物理按键并发变化不在软件保证内。

## dev 合并交付验证（当前发布依据）

用户明确授权合并到 dev 并 push。交付提交 `b4f095c` 合并到 dev；原工作区的焦点居中实验定义、两个 PoC 文件和锁文件处理脚本未提交、未删除。既有分支祖先中的正式功能提交随分支合并，不重写历史。

在独立 `win-mouse-fix-dev-verify-20261015` worktree 验证实际发布源码；Rust 离线重新构建，Node 依赖通过 junction 复用本机已有 `node_modules`，未复制原工作区的未发布源码/PoC/运行配置。初次格式检查发现隔离 focus-center 文件的空行问题，格式化后重新执行完整入口并通过。

入口：`powershell -NoProfile -ExecutionPolicy Bypass -File tools/drag-gauntlet.ps1`，退出码 0。以下结果均来自最后一次完整运行，之后仅补记文档及提交：

| 验证 | 发布源码观察结果 |
|---|---|
| 格式、all-target check、strict Clippy、debug 构建 | 全部通过 |
| `cargo test --offline --all-targets` | 242 passed，5 ignored，0 failed；旧 270 计数包含未发布的本地 PoC，不能用于此交付版本 |
| `npx --no-install playwright test` | 30 passed |
| 原生拖选与所有权隔离 | 自有 EDIT 整句选择、重复/旧 id、外来会话隔离通过 |
| 租期 / 断线 / 半帧释放 | 实际 up 分别在 2.038s / 0.052s / 2.046s 到达 |
| 服务停止/重启与正常进程退出 | 释放通过，新会话可使用；持有左键时退出观察到 up、左键状态清零，退出码 0 |
| 隔离变异检查 | 3/3 被行为测试击杀，基线和恢复后通过 |
| 覆盖诊断 | 11 个远程输入测试通过；不是 100% 变更行门禁 |

待发布源码/验收输入 SHA-256：`ec54ef1b8b33fe86ee4700b2368f18c27f873bebbbd5b8984be5d442adcf22d8`，复算命令与摘要范围同上。对选定源码、测试、工具、`package-lock.json`、跟踪配置及补记前证据做常见 token/私钥/带密码 URL 模式检查，无匹配；不等于完整安全审计。未安装依赖、未改防火墙，临时应用仍仅使用其独立配置/token；原工作区的本地文件保留。P2 真机边界和保证降级不变。
