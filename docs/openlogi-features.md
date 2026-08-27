# OpenLogi 功能清单

> 参考竞品分析。基于 `D:/work_space/personal_workspace/OpenLogi` 仓库源码整理。
> 最后更新：2026-08-27

---

## 一、设备支持与协议层

**支持的设备类型**：`Mouse`, `Keyboard`, `Numpad`, `Presenter`, `Remote`, `Trackball`, `Touchpad`, `Tablet`, `Gamepad`, `Joystick`, `Headset`, `Camera`, `Light`, `Unknown`

**协议**：HID++ 2.0 over USB/Bluetooth，通过 Bolt/Unifying/Lightspeed 接收器或蓝牙直连

**设备发现**：接收器插槽探测（最多 6 个 Bolt 插槽），hotplug 事件监听，离线设备缓存

---

## 二、电量系统

| Feature ID | Feature 名称 | 实现情况 | 备注 |
|---|---|---|---|
| `0x1004` | UnifiedBattery | ✅ 完整实现 | 现代设备：百分比 + Level + Status + 事件订阅 |
| `0x1000` | BatteryStatus（Legacy） | ✅ 完整实现 | MX Master 2S 等老设备：百分比 + 状态枚举 |
| `0x1001` | BatteryVoltage | ✅ 完整实现 | G 系列 gaming：电压 mV，无百分比 |
| `0x4301` | SolarKeyboardDashboard | ✅ 完整实现 | K750 等太阳能键盘：电量 + 光照 lux |

### BatteryInfo 结构

```rust
pub struct BatteryInfo {
    pub charging_percentage: u8,   // 电量百分比
    pub level: BatteryLevel,       // 等级：Critical/Low/Good/Full/Unknown
    pub status: BatteryStatus,     // 充电状态
}
```

### BatteryStatus 枚举

```rust
pub enum BatteryStatus {
    Discharging = 0,
    Charging = 1,
    ChargingNearlyFull = 2,
    Full = 3,
    ChargingSlow = 4,
    InvalidBattery = 5,
    ThermalError = 6,
    ChargingError = 7,
}
```

### 架构

- 探测阶段选择最优 battery feature（优先级 `0x1004 → 0x1000 → 0x1001`）
- 结果缓存复用，避免重复 HID++ 往返
- 支持设备主动上报电量事件（无需轮询）

---

## 三、按键 / 控件编程

**Feature**：`0x1b04` ReprogControlsV5（支持 `0x1b00–0x1b04` 降级）

### 核心 API

| 方法 | 说明 |
|---|---|
| `getCount()` | 获取设备控件表行数 |
| `getCidInfo(index)` | 读取单个控件信息 |
| `getCidReporting(cid)` | 读取控件当前 diversion / remap 状态 |
| `setCidReporting(cid, change)` | 写入 diversion / remap 变更 |
| `reset_all_cid_report_settings()` | 重置所有控件设置 |

### CidFlags 能力标志（bitflags）

```rust
pub struct CidFlags: u16 {
    const MOUSE = 1 << 0;              // 鼠标控件
    const FUNCTION_KEY = 1 << 1;       // 键盘功能键
    const HOTKEY = 1 << 2;             // 热键
    const FN_TOGGLE = 1 << 3;          // Fn 切换
    const REPROGRAMMABLE = 1 << 4;      // 可重编程
    const DIVERTABLE = 1 << 5;         // 可临时 diversion
    const PERSISTENTLY_DIVERTABLE = 1 << 6; // 可持久 diversion
    const VIRTUAL_CONTROL = 1 << 7;     // 虚拟控件
    const RAW_XY = 1 << 8;            // 支持原始 XY（手势）
    const FORCE_RAW_XY = 1 << 9;       // 强制原始 XY
    const ANALYTICS_KEY_EVENTS = 1 << 10; // 支持分析按键事件
    const RAW_WHEEL = 1 << 11;         // 支持原始滚轮
}
```

### CidReporting 状态结构

```rust
pub struct CidReporting {
    pub cid: ControlId,
    pub diverted: bool,                  // 临时 diversion 是否启用
    pub persistently_diverted: bool,    // 持久 diversion
    pub force_raw_xy: bool,
    pub raw_xy: bool,                   // 原始 XY 报告
    pub remap: Option<ControlId>,      // remap 目标控件
    pub analytics_key_events: bool,
    pub raw_wheel: bool,
}
```

### Remap 写入结构

```rust
pub struct CidReportingChange {
    pub diverted: Option<bool>,
    pub persistently_diverted: Option<bool>,
    pub force_raw_xy: Option<bool>,
    pub raw_xy: Option<bool>,
    pub remap: Option<ControlId>,   // None = 保持不变
    pub analytics_key_events: Option<bool>,
    pub raw_wheel: Option<bool>,
}
```

### Remap 流程

通过 `setCidReporting(cid, CidReportingChange { remap: Some(target_cid), .. })` 将物理按键 remap 到另一个控件 ID。

### 手势捕获会话（session/gesture.rs）

保持一个 HID++ channel 持续监听 diversion 的控件，转换为 `CapturedInput` 供 GUI 绑定到具体 Action：

```rust
pub enum CapturedInput {
    ButtonDown(ButtonId),
    ButtonUp(ButtonId),
    Gesture(ButtonId, GestureDirection),  // 滑动方向
    Scroll(i16, i16),                    // 滚轮/拇指轮 delta
}
```

---

## 四、DPI / 指针

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x2201` | AdjustableDpi | ✅ |
| `0x2202` | ExtendedAdjustableDpi | ✅ |
| `0x2240` | SurfaceTuning | — |
| `0x2200` | MousePointer | ✅ |

---

## 五、滚轮

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x2110` | SmartShiftWheel | ✅ |
| `0x2111` | SmartShiftWheelEnhanced | ✅ |
| `0x2120` | HighResolutionScrolling | — |
| `0x2121` | HiResWheel | ✅（支持反转 `scroll_inversion`） |
| `0x2150` | Thumbwheel | ✅ |
| `0x2100` | VerticalScrolling | ✅ |
| `0x2130` | RatchetWheel | — |

**SmartShift**：可配置滚轮模式（自由旋转 / 点击吸附），支持 `get_mode`/`set_mode`

**HiResWheel**：`get_wheel_movement()`, `get_wheel_movement_raw()`, `set_rotation` 高分辨率

---

## 六、背光 / 灯光

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x1982` / `0x1983` | Backlight2/3 | ✅（背光亮度控制） |
| `0x1990` | Illumination | ✅（键盘照明） |
| `0x8070` | ColorLedEffects | ✅ |
| `0x8071` | RgbEffects | ✅ |
| `0x8080` | PerKeyLighting | — |
| `0x8081` | PerKeyLighting2 | ✅ |

---

## 七、Haptic / 触觉反馈

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x19b0` | HapticFeedback | ✅ |
| `0x19c0` | ForceSensingButton | —（逆向工程中） |

**HapticFeedback**：`set_effect(effect)`, `get_effect()`

**Haptic Panel**（MX Master 4 拇指区触感面板）：作为 `0x1b04` 的特殊 Control ID (`0x01a0`) 被探测，支持 diversion + raw XY 报告

---

## 八、主机切换

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x1814` | ChangeHost | ✅ |
| `0x1815` | HostsInfo | ✅ |

---

## 九、Crown 旋钮（MX Master 3 等）

**Feature**：`0x4600` Crown

| 方法 | 说明 |
|---|---|
| `get_info()` | 获取能力、slots、ratchets 数量 |
| `get_mode()` / `set_mode()` | 获取/设置旋钮模式 |
| `get_cw_movement()` / `ccw_movement()` | 读取顺/逆时针旋转 |

**事件**：`CrownEvent::Update(CrownUpdate)` — rotation delta + pressed state

---

## 十、Gestures 手势

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x6500` | Gestures1 | — |
| `0x6501` | Gestures2 | ✅ |

**Gestures2**：`get_gestures_enabled()`, `get_gesture()`, `set_gesture()`

---

## 十一、其他配置功能

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x2001` | SwapLeftRightButton | ✅ |
| `0x40a0` | FnInversion | ✅ |
| `0x40a2` | FnInversionWithDefaultState | ✅ |
| `0x40a3` | FnInversionForMultiHostDevices | ✅ |
| `0x4521` | DisableKeys | ✅ |
| `0x4522` | DisableKeysByUsage | ✅ |
| `0x4530` | DualPlatform | ✅ |
| `0x4531` | MultiPlatform | ✅ |
| `0x1f1f` | FirmwareProperties | — |
| `0x1c00` | PersistentRemappableAction | ✅ |
| `0x1d4b` | WirelessDeviceStatus | ✅ |

---

## 十二、Onboard / Profile

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x8100` | OnboardProfiles | 部分（read/write profiles） |
| `0x8110` | MouseButtonFilter | — |

---

## 十三、音频相关

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x8300` | Sidetone | ✅（耳机侧音） |
| `0x8310` | Equalizer | ✅ |
| `0x8320` | HeadsetOut | — |

---

## 十四、Touchpad

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x6100` | TouchpadRawXy | ✅ |
| `0x6110` | TouchMouseRawTouchPoints | ✅ |

---

## 十五、Device Info / 身份

| Feature ID | Feature | 实现 |
|---|---|---|
| `0x0003` | DeviceInformation | ✅ |
| `0x0005` | DeviceTypeAndName | ✅ |
| `0x0007` | DeviceFriendlyName | ✅ |
| `0x0001` | FeatureSet | ✅ |
| `0x0000` | Root | ✅ |

---

## 十六、不支持的已知 Feature（未实现）

`WirelessSignalStrength(0x0080)`, `TouchpadRawXy`, `Gestures1`, `OnboardProfiles` 写入侧, `ReportRate` 完整读写, `MacroRecord`, `GamingGKeys`, `GamingMKeys`, `ForceFeedback`, `SensorAngleSnapping`, `XyStats`, `WheelStats`

---

## 十七、完整 Feature ID 注册表

```
0x0000 Root                        ✅
0x0001 FeatureSet                  ✅
0x0002 FeatureInfo                 
0x0003 DeviceInformation           ✅
0x0004 UnitId                     
0x0005 DeviceTypeAndName           ✅
0x0006 DeviceGroups                
0x0007 DeviceFriendlyName          ✅
0x0008 KeepAlive                   
0x0020 ConfigChange               
0x0021 UniqueRandomId             
0x0030 TargetSoftware             
0x0080 WirelessSignalStrength      
0x00c0-c3 DfuControl              （DFU 相关）
0x00d0-d1 Dfu                     
0x1000 BatteryStatus              ✅（Legacy）
0x1001 BatteryVoltage             ✅
0x1004 UnifiedBattery             ✅
0x1010 ChargingControl            
0x1300 LedControl                 
0x1800 GenericTest                
0x1802 DeviceReset                
0x1805 OobState                   
0x1806 ConfigDeviceProps           
0x1814 ChangeHost                 ✅
0x1815 HostsInfo                   ✅
0x1981 Backlight1                 
0x1982 Backlight2                 ✅
0x1983 Backlight3                 
0x1990 Illumination               ✅
0x19b0 HapticFeedback              ✅
0x19c0 ForceSensingButton         —（逆向中）
0x1a00 PresenterControl           
0x1a01 Sensor3D                   
0x1b00 ReprogControls             
0x1b01 ReprogControls2            
0x1b02 ReprogControls3            
0x1b03 ReprogControls4            
0x1b04 ReprogControls5             ✅（核心）
0x1bc0 ReportHidUsages            
0x1c00 PersistentRemappableAction ✅
0x1d4b WirelessDeviceStatus       ✅
0x1df0 RemainingPairings          
0x1f1f FirmwareProperties         
0x1f20 AdcMeasurement             
0x2001 SwapLeftRightButton        ✅
0x2005 ButtonSwapCancel           
0x2006 PointerAxesOrientation     
0x2100 VerticalScrolling          ✅
0x2110 SmartShiftWheel            ✅
0x2111 SmartShiftWheelEnhanced    ✅
0x2120 HighResolutionScrolling   
0x2121 HiResWheel                ✅
0x2130 RatchetWheel              
0x2150 Thumbwheel                 ✅
0x2200 MousePointer               ✅
0x2201 AdjustableDpi             ✅
0x2202 ExtendedAdjustableDpi     ✅
0x2205 PointerMotionScaling       
0x2230 SensorAngleSnapping        
0x2240 SurfaceTuning             
0x2250 XyStats                   
0x2251 WheelStats                
0x2400 HybridTrackingEngine       
0x40a0 FnInversion               ✅
0x40a2 FnInversionWithDefaultState ✅
0x40a3 FnInversionForMultiHostDevices ✅
0x4100 Encryption                 
0x4220 LockKeyState              
0x4301 SolarKeyboardDashboard    ✅
0x4520 KeyboardLayout             
0x4521 DisableKeys               ✅
0x4522 DisableKeysByUsage        ✅
0x4530 DualPlatform              ✅
0x4531 MultiPlatform            ✅
0x4540 KeyboardInternationalLayouts
0x4600 Crown                     ✅
0x6010-2 TouchpadFw/Sw/Win8    
0x6020-1 TapEnable               
0x6030 CursorBallistic           
0x6040 TouchpadResolutionDivider 
0x6100 TouchpadRawXy             ✅
0x6110 TouchMouseRawTouchPoints  ✅
0x6120 BtTouchMouseSettings      
0x6500 Gestures1                 
0x6501 Gestures2                 ✅
0x8010 GamingGKeys              
0x8020 GamingMKeys              
0x8030 MacroRecord               
0x8040 BrightnessControl         ✅
0x8060 AdjustableReportRate      ✅
0x8061 ExtendedAdjustableReportRate ✅
0x8070 ColorLedEffects           ✅
0x8071 RgbEffects               ✅
0x8080 PerKeyLighting           
0x8081 PerKeyLighting2           ✅
0x8090 ModeStatus                ✅
0x8100 OnboardProfiles           部分
0x8110 MouseButtonFilter        
0x8111 LatencyMonitoring         
0x8120 GamingAttachments        
0x8123 ForceFeedback             
0x8300 Sidetone                 ✅
0x8310 Equalizer                ✅
0x8320 HeadsetOut               
```

---

## 十八、架构亮点（可借鉴）

### 1. Feature Registry 宏
所有 HID++ Feature ID 集中注册在 `known_features!` 宏中，编译时校验 Feature ID 与实现类型匹配，typo 不可能注册到错误 ID。

### 2. Feature 派生宏（openlogi-hidpp-derive）
```rust
#[derive(Feature)]
#[creatable(id = 0x1004, version = 0)]
pub struct UnifiedBatteryFeature {
    endpoint: FeatureEndpoint,
    events: EventSource<BatteryEvent>,
}
```
零样板代码注册新 Feature，自动实现 `CreatableFeature`。

### 3. Device Probe 模式
探测阶段读特征表，再按需读具体能力，支持部分能力缺失的优雅降级，结果缓存复用。

### 4. EventSource + DecodeEvent
无需轮询的 Feature（电量、控件事件）通过 channel listener 接收广播事件，解耦订阅。

### 5. Capabilities DTO
```rust
pub struct Capabilities {
    pub buttons: bool,           // 0x1b00-0x1b04
    pub pointer: bool,           // 0x2201/0x2202
    pub lighting: bool,          // 0x8070/0x8080/0x8081
    pub scroll_inversion: bool,  // 0x2121 invert
    pub hires_wheel: bool,       // 0x2121
    pub thumbwheel: bool,        // 0x2150
    pub haptic_feedback: bool,   // 0x19b0
    pub haptic_panel: bool,      // 0x1b04 特殊控件
}
```
从 Feature ID 集合派生，UI 根据 capability 显示/隐藏面板，而不是根据 DeviceKind。

### 6. 多接收器支持
Bolt/Unifying/Lightspeed 三套接收器协议并存，插槽探测并发进行（13s probe budget）。

---

## 十九、对 win-mouse-fix 的参考价值

### 电量获取
- 参考 `unified_battery.rs` 的 `BatteryInfo` 结构设计
- 参考 `inventory/features.rs` 的多 Feature 降级探测策略
- 可用事件订阅替代轮询

### 按键绑定
- 参考 `reprog_controls.rs` 的 `CidReporting`/`CidReportingChange` 设计
- 参考 `session/gesture.rs` 的 diversion 会话保持 + restore 机制
- `remap` 字段是持久化 remap 的关键

### Feature 扩展
- 参考 `openlogi-hidpp-derive` 的派生宏，避免每个新 Feature 写样板代码
- 参考 `registry.rs` 的 `known_features!` 宏集中管理 Feature ID
