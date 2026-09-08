//! Logitech HID++ 2.0 protocol (pure, testable).
//!
//! HID++ messages travel as HID feature reports. Short reports use Report ID
//! `0x10` (7 bytes including the report id); long reports use `0x11` (20 bytes).
//! See the public HID++ 2.0 specification for the message framing.
//!
//! Feature indices are 16-bit but short messages only carry the high byte in
//! byte 2; the low byte is 0 for all standard features (`0x00xx`).

// Protocol surface kept for the device-layer roadmap (Phase 8 maturity); not
// all of it is consumed by the battery closed loop yet.
#![allow(dead_code)]

pub const REPORT_ID_SHORT: u8 = 0x10;
pub const REPORT_ID_LONG: u8 = 0x11;

/// Well-known HID++ feature ids.
pub mod feature {
    pub const ROOT: u16 = 0x0000;
    pub const DEVICE_INFO: u16 = 0x0003;
    pub const BATTERY: u16 = 0x1000;
    pub const BATTERY_VOLTAGE: u16 = 0x1001;
    pub const UNIFYING_PAIRING: u16 = 0x0020;
    /// Adjustable DPI (0x2201): read/set the sensor resolution.
    pub const ADJUSTABLE_DPI: u16 = 0x2201;
}

/// Software id (low nibble of the function/software-id byte). Logitech reserves
/// `0..=0x0F`; `0x05` is conventional for first-party software.
pub const SOFTWARE_ID: u8 = 0x05;

/// A short (7-byte) HID++ message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortMessage {
    pub device_index: u8,
    pub feature_index: u16,
    pub function_id: u8,
    pub software_id: u8,
    pub params: [u8; 3],
}

/// A long (20-byte) HID++ message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LongMessage {
    pub device_index: u8,
    pub feature_index: u16,
    pub function_id: u8,
    pub software_id: u8,
    pub params: [u8; 16],
}

impl ShortMessage {
    pub fn encode(&self) -> [u8; 7] {
        let mut b = [0u8; 7];
        b[0] = REPORT_ID_SHORT;
        b[1] = self.device_index;
        b[2] = (self.feature_index >> 8) as u8;
        b[3] = (self.function_id << 4) | (self.software_id & 0x0F);
        b[4] = self.params[0];
        b[5] = self.params[1];
        b[6] = self.params[2];
        b
    }

    pub fn decode(b: &[u8]) -> Option<ShortMessage> {
        if b.len() < 7 || b[0] != REPORT_ID_SHORT {
            return None;
        }
        Some(ShortMessage {
            device_index: b[1],
            feature_index: (b[2] as u16) << 8,
            function_id: b[3] >> 4,
            software_id: b[3] & 0x0F,
            params: [b[4], b[5], b[6]],
        })
    }
}

impl LongMessage {
    pub fn encode(&self) -> [u8; 20] {
        let mut b = [0u8; 20];
        b[0] = REPORT_ID_LONG;
        b[1] = self.device_index;
        b[2] = (self.feature_index >> 8) as u8;
        b[3] = (self.function_id << 4) | (self.software_id & 0x0F);
        b[4..20].copy_from_slice(&self.params);
        b
    }

    pub fn decode(b: &[u8]) -> Option<LongMessage> {
        if b.len() < 20 || b[0] != REPORT_ID_LONG {
            return None;
        }
        let mut params = [0u8; 16];
        params.copy_from_slice(&b[4..20]);
        Some(LongMessage {
            device_index: b[1],
            feature_index: (b[2] as u16) << 8,
            function_id: b[3] >> 4,
            software_id: b[3] & 0x0F,
            params,
        })
    }
}

/// Build a ROOT-feature `GetFeature` request to resolve a feature id to its
/// feature index. `feature_id` is the 16-bit feature id (e.g. `feature::BATTERY`).
pub fn get_feature_index_request(device_index: u8, feature_id: u16) -> ShortMessage {
    ShortMessage {
        device_index,
        feature_index: feature::ROOT,
        function_id: 0x00,
        software_id: SOFTWARE_ID,
        params: [(feature_id >> 8) as u8, (feature_id & 0xFF) as u8, 0],
    }
}

/// Extract the resolved feature index from a `GetFeature` response, or `None`
/// if the feature was not found (response param0 == 0).
pub fn feature_index_from_response(msg: &ShortMessage) -> Option<u8> {
    if msg.feature_index == feature::ROOT && msg.function_id == 0x00 && msg.params[0] != 0 {
        Some(msg.params[0])
    } else {
        None
    }
}

/// Battery level/status from the `BATTERY` (0x1000) feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatteryStatus {
    /// 0..=100, or 0xFF if the device reports no level.
    pub level: u8,
    pub charging: bool,
    /// `true` when `level == 0xFF` (device did not report a usable level).
    pub level_invalid: bool,
}

/// Build a `GetBatteryLevelStatus` (function 0x00) request for a resolved
/// battery feature index.
pub fn battery_level_request(device_index: u8, battery_feature_index: u8) -> ShortMessage {
    ShortMessage {
        device_index,
        feature_index: (battery_feature_index as u16) << 8,
        function_id: 0x00,
        software_id: SOFTWARE_ID,
        params: [0, 0, 0],
    }
}

/// Parse a `GetBatteryLevelStatus` response.
pub fn battery_status_from_response(msg: &ShortMessage) -> Option<BatteryStatus> {
    if msg.function_id != 0x00 {
        return None;
    }
    let level = msg.params[0];
    let charging = (msg.params[1] & 0x01) != 0;
    let level_invalid = level == 0xFF;
    Some(BatteryStatus {
        level,
        charging,
        level_invalid,
    })
}

/// Build a `GetSensorDpi` (function 0x00) request for the adjustable-DPI
/// feature. `sensor` selects the sensor (0 = primary).
///
/// NOTE: HID++ short-message function ids occupy the high nibble of byte 3, so
/// they are limited to 0x0–0xF (see `ShortMessage::encode`). The Logitech 0x2201
/// "Adjustable DPI" feature documents `GetSensorDpi` at function 0x00; confirm
/// the exact index against a real device before trusting the write path.
pub fn get_dpi_request(device_index: u8, dpi_feature_index: u8, sensor: u8) -> ShortMessage {
    ShortMessage {
        device_index,
        feature_index: (dpi_feature_index as u16) << 8,
        function_id: 0x00,
        software_id: SOFTWARE_ID,
        params: [sensor, 0, 0],
    }
}

/// Parse a `GetSensorDpi` response. Resolution is `param1 | (param2 << 8)`.
pub fn dpi_from_response(msg: &ShortMessage) -> Option<u16> {
    if msg.function_id != 0x00 {
        return None;
    }
    Some((msg.params[1] as u16) | ((msg.params[2] as u16) << 8))
}

/// Build a `SetSensorDpi` request. `dpi` low byte → param1, high byte → param2.
/// Confirmed against OpenLogi hidpp crate (openlogi-hidpp/src/feature/adjustable_dpi.rs):
/// Feature 0x2201 AdjustableDpi function codes: 0=GetSensorCount, 1=GetSensorDpiList,
/// 2=GetSensorDpi, 3=SetSensorDpi. DPI bytes are BIG-ENDIAN (hi, lo) per OpenLogi.
pub fn set_dpi_request(
    device_index: u8,
    dpi_feature_index: u8,
    sensor: u8,
    dpi: u16,
) -> ShortMessage {
    ShortMessage {
        device_index,
        feature_index: (dpi_feature_index as u16) << 8,
        function_id: 0x03, // SetSensorDpi (confirmed from OpenLogi)
        software_id: SOFTWARE_ID,
        params: [sensor, (dpi >> 8) as u8, (dpi & 0xFF) as u8], // big-endian hi/lo
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_roundtrip() {
        let m = ShortMessage {
            device_index: 0x01,
            feature_index: 0x0500,
            function_id: 0x03,
            software_id: 0x05,
            params: [0xAA, 0xBB, 0xCC],
        };
        let enc = m.encode();
        assert_eq!(enc[0], 0x10);
        assert_eq!(enc[2], 0x05); // high byte of feature index
        assert_eq!(enc[3], 0x35); // function 3 << 4 | software 5
        assert_eq!(ShortMessage::decode(&enc), Some(m));
    }

    #[test]
    fn feature_index_request_encoding() {
        let enc = get_feature_index_request(0x01, feature::BATTERY).encode();
        assert_eq!(enc, [0x10, 0x01, 0x00, 0x05, 0x10, 0x00, 0x00]);

        let resp = ShortMessage::decode(&[0x10, 0x01, 0x00, 0x05, 0x05, 0x01, 0x00]).unwrap();
        assert_eq!(feature_index_from_response(&resp), Some(0x05));

        let missing = ShortMessage::decode(&[0x10, 0x01, 0x00, 0x05, 0x00, 0x00, 0x00]).unwrap();
        assert_eq!(feature_index_from_response(&missing), None);
    }

    #[test]
    fn battery_response_parsing() {
        let full = ShortMessage::decode(&[0x10, 0x01, 0x00, 0x05, 0x64, 0x00, 0x00]).unwrap();
        assert_eq!(
            battery_status_from_response(&full),
            Some(BatteryStatus {
                level: 100,
                charging: false,
                level_invalid: false
            })
        );

        let charging = ShortMessage::decode(&[0x10, 0x01, 0x00, 0x05, 0x32, 0x01, 0x00]).unwrap();
        assert_eq!(
            battery_status_from_response(&charging),
            Some(BatteryStatus {
                level: 50,
                charging: true,
                level_invalid: false
            })
        );

        let invalid = ShortMessage::decode(&[0x10, 0x01, 0x00, 0x05, 0xFF, 0x00, 0x00]).unwrap();
        assert_eq!(
            battery_status_from_response(&invalid),
            Some(BatteryStatus {
                level: 0xFF,
                charging: false,
                level_invalid: true
            })
        );

        assert_eq!(
            battery_level_request(0x01, 0x05).encode(),
            [0x10, 0x01, 0x05, 0x05, 0x00, 0x00, 0x00]
        );
    }

    #[test]
    fn long_roundtrip() {
        let m = LongMessage {
            device_index: 0x02,
            feature_index: 0x0100,
            function_id: 0x01,
            software_id: 0x00,
            params: [
                0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
                0xFF, 0x00,
            ],
        };
        let enc = m.encode();
        assert_eq!(enc[0], 0x11);
        assert_eq!(LongMessage::decode(&enc), Some(m));
    }

    #[test]
    fn dpi_get_set_roundtrip() {
        let get = get_dpi_request(0x01, 0x07, 0).encode();
        assert_eq!(get, [0x10, 0x01, 0x07, 0x05, 0x00, 0x00, 0x00]);
        // Get echoes function 0x00; response dpiLow 0x40, dpiHigh 0x01 => 320.
        let resp = ShortMessage::decode(&[0x10, 0x01, 0x07, 0x05, 0x00, 0x40, 0x01]).unwrap();
        assert_eq!(dpi_from_response(&resp), Some(320));

        let set = set_dpi_request(0x01, 0x07, 0, 320).encode();
        // fn_id=0x03 (SetSensorDpi), big-endian DPI bytes: hi=0x01, lo=0x40
        assert_eq!(set, [0x10, 0x01, 0x07, 0x35, 0x00, 0x01, 0x40]);
    }
}
