//! Diagnostic: polls GetAsyncKeyState for middle button and all other buttons.
use std::thread;
use std::time::Duration;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;

fn main() {
    println!("=== Middle Button Diagnostic ===");
    println!("Press mouse buttons. If MIDDLE shows DOWN but hook doesn't see it,");
    println!("the driver is intercepting at a level below WH_MOUSE_LL.\n");

    let vk_names: [(u32, &str); 6] = [
        (VK_LBUTTON as u32, "LBtn"),
        (VK_RBUTTON as u32, "RBtn"),
        (VK_MBUTTON as u32, "MBtn"),
        (VK_XBUTTON1 as u32, "X1"),
        (VK_XBUTTON2 as u32, "X2"),
        (0x05, "XBtn3"), // Some mice use 0x05 for middle
    ];

    let mut prev_states = vec![false; vk_names.len()];

    loop {
        for (i, &(vk, name)) in vk_names.iter().enumerate() {
            let state = unsafe { GetAsyncKeyState(vk as i32) };
            let down = (state as u16 & 0x8000) != 0;
            if down != prev_states[i] {
                println!("{}: {}", name, if down { "DOWN" } else { "UP" });
                prev_states[i] = down;
            }
        }

        // Also check raw scan codes for middle button
        // VK_MBUTTON = 0x04
        let m_state = unsafe { GetAsyncKeyState(0x04) };
        let _m_down = (m_state as u16 & 0x8000) != 0;

        thread::sleep(Duration::from_millis(20));
    }
}
