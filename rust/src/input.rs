//! Linux joystick API (`/dev/input/js*`) for Retro-bit wireless receivers.

use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::io::AsRawFd;
use std::path::Path;

const JS_EVENT_SIZE: usize = 8;
const JS_EVENT_BUTTON: u8 = 0x01;
const JS_EVENT_AXIS: u8 = 0x02;
const JS_EVENT_INIT: u8 = 0x80;

const AXIS_MAX: f32 = 32767.0;
const STICK_DEADZONE: f32 = 8000.0;

/// Best-guess D-Input map for Retro-bit 2563:0575 (A2 receiver).
/// D-pad on axes 4/5 confirmed on device; buttons from LEGACY16 manual.
#[derive(Debug, Clone, Default)]
pub struct PadState {
    pub dpad_up: bool,
    pub dpad_down: bool,
    pub dpad_left: bool,
    pub dpad_right: bool,
    pub btn_a: bool,
    pub btn_b: bool,
    pub btn_x: bool,
    pub btn_y: bool,
    pub btn_l: bool,
    pub btn_r: bool,
    pub btn_zl: bool,
    pub btn_zr: bool,
    pub btn_select: bool,
    pub btn_start: bool,
    pub btn_l3: bool,
    pub btn_r3: bool,
    pub btn_home: bool,
    pub btn_capture: bool,
    pub left_x: f32,
    pub left_y: f32,
    pub right_x: f32,
    pub right_y: f32,
    pub last_raw: String,
    /// Human-readable list of everything currently held.
    pub pressed_now: String,
    /// Friendly label for the most recent event (for mapping debug).
    pub last_event_label: String,
}

pub struct PadReader {
    file: File,
    pub path: String,
    pub info: PadInfo,
    state: PadState,
}

/// Static metadata read from sysfs / ioctl when the pad is opened.
#[derive(Debug, Clone, Default)]
pub struct PadInfo {
    pub name: String,
    pub vendor_id: String,
    pub product_id: String,
    pub serial: String,
    pub phys: String,
    /// USB topology port, e.g. `1-1.3` from sysfs path / `usb-…-1.3` from phys.
    pub usb_port: String,
    pub js_buttons: u8,
    pub js_axes: u8,
    pub evdev_key_count: usize,
    pub evdev_abs_count: usize,
    /// False when the kernel js API exposes no button slot beyond Home (js 12).
    pub capture_available: bool,
}

impl PadReader {
    /// Open the A2 Retro-bit receiver, or fall back to js2 / first js node.
    pub fn open() -> io::Result<Self> {
        const CANDIDATES: &[&str] = &[
            "/dev/input/by-id/usb-SWITCH_CO._LTD._Retro-bit_Controller_GH-SP-5027-1_A2-joystick",
            "/dev/input/js2",
            "/dev/input/js1",
            "/dev/input/js0",
        ];
        for path in CANDIDATES {
            if Path::new(path).exists() {
                if let Ok(reader) = Self::open_path(path) {
                    return Ok(reader);
                }
            }
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no joystick device found",
        ))
    }

    pub fn open_path(path: &str) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).open(path)?;
        set_nonblocking(&file)?;
        let info = read_pad_info(path, &file)?;
        eprintln!(
            "pad: opened {path} ({info})",
            info = format!(
                "{} usb={} {} btn={} axes={}",
                info.name, info.usb_port, info.vendor_id, info.js_buttons, info.js_axes
            )
        );
        Ok(Self {
            path: path.to_string(),
            file,
            info,
            state: PadState::default(),
        })
    }

    pub fn info(&self) -> &PadInfo {
        &self.info
    }

    pub fn state(&self) -> &PadState {
        &self.state
    }

    /// Drain pending events; returns true if state changed.
    pub fn poll(&mut self) -> bool {
        let mut buf = [0u8; JS_EVENT_SIZE];
        let mut changed = false;
        loop {
            match self.file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) if n < JS_EVENT_SIZE => break,
                Ok(_) => {
                    let event_type = buf[6] & !JS_EVENT_INIT;
                    let number = buf[7];
                    let value = i16::from_le_bytes([buf[4], buf[5]]);
                    if self.state.apply_event(event_type, number, value) {
                        changed = true;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    eprintln!("pad read error: {e}");
                    break;
                }
            }
        }
        changed
    }
}

impl PadState {
    fn apply_event(&mut self, event_type: u8, number: u8, value: i16) -> bool {
        self.last_raw = format!("type={event_type} num={number} val={value}");
        let changed = match event_type {
            JS_EVENT_BUTTON => {
                let pressed = value != 0;
                let label = match number {
                    // Retro-bit A2 SNES: js indices don't match textbook D-input
                    // (0=B,1=A,2=X,3=Y). Physical SNES labels map as:
                    //   js0=Y  js1=B  js2=A  js3=X
                    0 => {
                        self.btn_y = pressed;
                        "Y"
                    }
                    1 => {
                        self.btn_b = pressed;
                        "B"
                    }
                    2 => {
                        self.btn_a = pressed;
                        "A"
                    }
                    3 => {
                        self.btn_x = pressed;
                        "X"
                    }
                    4 => {
                        self.btn_l = pressed;
                        "L"
                    }
                    5 => {
                        self.btn_r = pressed;
                        "R"
                    }
                    6 => {
                        self.btn_zl = pressed;
                        "ZL"
                    }
                    7 => {
                        self.btn_zr = pressed;
                        "ZR"
                    }
                    8 => {
                        self.btn_select = pressed;
                        "Select"
                    }
                    9 => {
                        self.btn_start = pressed;
                        "Start"
                    }
                    10 => {
                        self.btn_l3 = pressed;
                        "L3"
                    }
                    11 => {
                        self.btn_r3 = pressed;
                        "R3"
                    }
                    12 => {
                        self.btn_home = pressed;
                        "Home"
                    }
                    13 => {
                        self.btn_capture = pressed;
                        "Capture"
                    }
                    _ => {
                        self.last_event_label = format!(
                            "unknown btn {number} {}",
                            if pressed { "down" } else { "up" }
                        );
                        self.rebuild_pressed_now();
                        return false;
                    }
                };
                self.last_event_label = format!(
                    "{label} {} (js btn {number})",
                    if pressed { "down" } else { "up" }
                );
                true
            }
            JS_EVENT_AXIS => {
                let v = value as f32;
                let label = match number {
                    0 => {
                        self.left_x = normalize_stick(v);
                        "Left X"
                    }
                    1 => {
                        self.left_y = normalize_stick(v);
                        "Left Y"
                    }
                    2 => {
                        self.right_x = normalize_stick(v);
                        "Right X"
                    }
                    3 => {
                        self.right_y = normalize_stick(v);
                        "Right Y"
                    }
                    4 => {
                        if v < -STICK_DEADZONE {
                            self.dpad_left = true;
                            self.dpad_right = false;
                        } else if v > STICK_DEADZONE {
                            self.dpad_right = true;
                            self.dpad_left = false;
                        } else {
                            self.dpad_left = false;
                            self.dpad_right = false;
                        }
                        "D-pad X"
                    }
                    5 => {
                        if v < -STICK_DEADZONE {
                            self.dpad_up = true;
                            self.dpad_down = false;
                        } else if v > STICK_DEADZONE {
                            self.dpad_down = true;
                            self.dpad_up = false;
                        } else {
                            self.dpad_up = false;
                            self.dpad_down = false;
                        }
                        "D-pad Y"
                    }
                    _ => {
                        self.last_event_label =
                            format!("unknown axis {number} val={value}");
                        self.rebuild_pressed_now();
                        return false;
                    }
                };
                self.last_event_label = format!("{label} axis {number} = {value}");
                true
            }
            _ => return false,
        };
        self.rebuild_pressed_now();
        changed
    }

    fn rebuild_pressed_now(&mut self) {
        let mut parts: Vec<&str> = Vec::new();
        if self.dpad_up {
            parts.push("D-Up");
        }
        if self.dpad_down {
            parts.push("D-Down");
        }
        if self.dpad_left {
            parts.push("D-Left");
        }
        if self.dpad_right {
            parts.push("D-Right");
        }
        macro_rules! btn {
            ($field:ident, $name:expr) => {
                if self.$field {
                    parts.push($name);
                }
            };
        }
        btn!(btn_y, "Y");
        btn!(btn_b, "B");
        btn!(btn_a, "A");
        btn!(btn_x, "X");
        btn!(btn_l, "L");
        btn!(btn_r, "R");
        btn!(btn_zl, "ZL");
        btn!(btn_zr, "ZR");
        btn!(btn_select, "Select");
        btn!(btn_start, "Start");
        btn!(btn_l3, "L3");
        btn!(btn_r3, "R3");
        btn!(btn_home, "Home");
        btn!(btn_capture, "Capture");
        if self.left_x.abs() > 0.01 || self.left_y.abs() > 0.01 {
            parts.push("Left stick");
        }
        if self.right_x.abs() > 0.01 || self.right_y.abs() > 0.01 {
            parts.push("Right stick");
        }
        self.pressed_now = if parts.is_empty() {
            "—".into()
        } else {
            parts.join(", ")
        };
    }
}

fn normalize_stick(v: f32) -> f32 {
    if v.abs() < STICK_DEADZONE {
        return 0.0;
    }
    (v / AXIS_MAX).clamp(-1.0, 1.0)
}

fn read_pad_info(js_path: &str, file: &File) -> io::Result<PadInfo> {
    use std::path::PathBuf;
    let js_node = resolve_js_node(js_path)?;
    let sys = PathBuf::from("/sys/class/input").join(&js_node).join("device");

    let read = |name: &str| -> String {
        std::fs::read_to_string(sys.join(name))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    let vendor_raw = read("id/vendor");
    let product_raw = read("id/product");
    let vendor_id = format_hex_id(&vendor_raw);
    let product_id = format_hex_id(&product_raw);
    let phys = read("phys");
    let usb_port = usb_port_from_sys(&sys)
        .or_else(|| usb_port_from_phys(&phys))
        .unwrap_or_else(|| "unknown".into());

    let key_words = read("capabilities/key");
    let abs_words = read("capabilities/abs");
    let evdev_key_count = count_capability_bits(&key_words);
    let evdev_abs_count = count_capability_bits(&abs_words);

    let js_buttons = js_ioctl_u8(file, 0x8001_6a12).unwrap_or(0); // JSIOCGBUTTONS
    let js_axes = js_ioctl_u8(file, 0x8001_6a11).unwrap_or(0); // JSIOCGAXES
    let name = read("name");

    // Measured on A2 receiver: JSIOCGBUTTONS=13 (js 0..12); Home=js12; no js slot for Capture.
    let capture_available = js_buttons > 13;

    Ok(PadInfo {
        name,
        vendor_id,
        product_id,
        serial: read("uniq"),
        phys,
        usb_port,
        js_buttons,
        js_axes,
        evdev_key_count,
        evdev_abs_count,
        capture_available,
    })
}

fn resolve_js_node(js_path: &str) -> io::Result<String> {
    let link = std::fs::read_link(js_path).or_else(|_| Path::new(js_path).canonicalize())?;
    let file_name = link
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| s.starts_with("js"))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a js device"))?;
    Ok(file_name)
}

fn format_hex_id(raw: &str) -> String {
    if raw.is_empty() {
        return String::new();
    }
    format!(
        "0x{:04x}",
        u16::from_str_radix(raw, 16).unwrap_or(0)
    )
}

fn usb_port_from_phys(phys: &str) -> Option<String> {
    // e.g. usb-ffb40000.usb-1.3/input0 — use as-is after the host controller prefix.
    let host = phys.split('/').next()?;
    let idx = host.rfind(".usb-")?;
    Some(host[idx + 5..].to_string())
}

fn usb_port_from_sys(sys: &Path) -> Option<String> {
    let full = std::fs::canonicalize(sys).ok()?;
    for part in full.components() {
        if let std::path::Component::Normal(s) = part {
            let s = s.to_str()?;
            if s.starts_with("1-1.") && s.len() > 4 {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn count_capability_bits(words: &str) -> usize {
    words
        .split_whitespace()
        .filter_map(|w| u32::from_str_radix(w, 16).ok())
        .map(|v| v.count_ones() as usize)
        .sum()
}

fn js_ioctl_u8(file: &File, req: libc::c_ulong) -> io::Result<u8> {
    let mut buf = [0u8];
    let rc = unsafe { libc::ioctl(file.as_raw_fd(), req, buf.as_mut_ptr()) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(buf[0])
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if rc < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Listen on js + evdev + hidraw for anything Capture might emit (lower layer than js API).
pub fn sniff(path: Option<&str>, secs: u64) -> io::Result<()> {
    let (js_path, info) = match path {
        Some(p) => {
            let f = OpenOptions::new().read(true).open(p)?;
            (p.to_string(), read_pad_info(p, &f)?)
        }
        None => {
            let r = PadReader::open()?;
            (r.path.clone(), r.info.clone())
        }
    };

    let event_path = resolve_event_path(&js_path)?;
    let hidraw_path = resolve_hidraw_path(&js_path);

    let mut js = OpenOptions::new().read(true).open(&js_path)?;
    let mut evdev = OpenOptions::new().read(true).open(&event_path)?;
    set_nonblocking(&js)?;
    set_nonblocking(&evdev)?;

    // Try to grab evdev so MiSTer can't swallow events (ignore failure).
    let grab: i32 = 1;
    let _ = unsafe {
        libc::ioctl(
            evdev.as_raw_fd(),
            0x4004_4590u32 as libc::c_ulong, // EVIOCGRAB
            &grab,
        )
    };

    let mut hidraw = hidraw_path
        .as_ref()
        .and_then(|p| OpenOptions::new().read(true).open(p).ok());

    if let Some(ref h) = hidraw {
        set_nonblocking(h)?;
    }

    eprintln!("sniff {secs}s on:");
    eprintln!("  js     {js_path}");
    eprintln!("  evdev  {event_path}");
    if let Some(ref h) = hidraw_path {
        eprintln!("  hidraw {h}");
    } else {
        eprintln!("  hidraw (not found)");
    }
    eprintln!(
        "  {} usb={} {} — press Capture (and anything else)…",
        info.name, info.usb_port, info.vendor_id
    );

    let start = std::time::Instant::now();
    let mut js_buf = [0u8; JS_EVENT_SIZE];
    let mut ev_buf = [0u8; 24];
    let mut hid_buf = [0u8; 64];
    let mut hid_idle: Option<[u8; 64]> = None;

    while start.elapsed().as_secs() < secs {
        let mut activity = false;

        match js.read(&mut js_buf) {
            Ok(n) if n >= JS_EVENT_SIZE => {
                let ty = js_buf[6] & !JS_EVENT_INIT;
                let num = js_buf[7];
                let val = i16::from_le_bytes([js_buf[4], js_buf[5]]);
                println!("[js]     {} {} = {val}", js_kind(ty), num);
                activity = true;
            }
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
            _ => {}
        }

        match evdev.read(&mut ev_buf) {
            Ok(n) if n >= 16 => {
                let (ty, code, val) = parse_input_event(&ev_buf[..n]);
                if ty != EV_SYN {
                    println!(
                        "[evdev]  {} code={} ({}) val={val}",
                        ev_type_name(ty),
                        code,
                        ev_code_name(ty, code)
                    );
                    activity = true;
                }
            }
            Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
            _ => {}
        }

        if let Some(ref mut h) = hidraw {
            match h.read(&mut hid_buf) {
                Ok(n) if n > 0 => {
                    let slice = &hid_buf[..n];
                    let changed = hid_idle
                        .map(|prev| prev[..n] != slice[..])
                        .unwrap_or(true);
                    if changed {
                        println!("[hidraw] {} bytes: {}", n, hex_bytes(slice));
                        hid_idle = Some({
                            let mut a = [0u8; 64];
                            a[..n].copy_from_slice(slice);
                            a
                        });
                        activity = true;
                    }
                }
                Err(e) if e.kind() != io::ErrorKind::WouldBlock => return Err(e),
                _ => {}
            }
        }

        if !activity {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    let grab_off: i32 = 0;
    let _ = unsafe {
        libc::ioctl(
            evdev.as_raw_fd(),
            0x4004_4590u32 as libc::c_ulong,
            &grab_off,
        )
    };
    Ok(())
}

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_REL: u16 = 2;
const EV_ABS: u16 = 3;
const EV_MSC: u16 = 4;

fn parse_input_event(buf: &[u8]) -> (u16, u16, i32) {
    // arm32 input_event: 8-byte timeval + type + code + value = 16 bytes
    if buf.len() >= 24 {
        (
            u16::from_le_bytes([buf[16], buf[17]]),
            u16::from_le_bytes([buf[18], buf[19]]),
            i32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]),
        )
    } else {
        (
            u16::from_le_bytes([buf[8], buf[9]]),
            u16::from_le_bytes([buf[10], buf[11]]),
            i32::from_le_bytes([buf[12], buf[13], buf[14], buf[15]]),
        )
    }
}

fn ev_type_name(ty: u16) -> &'static str {
    match ty {
        EV_SYN => "SYN",
        EV_KEY => "KEY",
        EV_REL => "REL",
        EV_ABS => "ABS",
        EV_MSC => "MSC",
        _ => "type",
    }
}

fn js_kind(ty: u8) -> &'static str {
    match ty {
        JS_EVENT_BUTTON => "btn",
        JS_EVENT_AXIS => "axis",
        _ => "?",
    }
}

fn ev_code_name(ty: u16, code: u16) -> String {
    match ty {
        EV_KEY => {
            const KEY: &[(u16, &str)] = &[
                (0x10, "KEY_Q/btn0"),
                (0x11, "KEY_W/btn1"),
                (0x12, "KEY_E/btn2"),
                (0x13, "KEY_R/btn3"),
                (0x14, "KEY_T/btn4"),
                (0x15, "KEY_Y/btn5"),
                (0x16, "KEY_U/btn6"),
                (0x17, "KEY_I/btn7"),
                (0x18, "KEY_O/btn8"),
                (0x19, "KEY_P/btn9"),
                (0x1a, "KEY_[/btn10"),
                (0x1b, "KEY_]/btn11"),
                (0x1c, "KEY_ENTER/btn12-Home"),
                (0x130, "BTN_0"),
                (0x131, "BTN_A"),
                (0x132, "BTN_B"),
                (0x133, "BTN_X"),
                (0x134, "BTN_Y"),
                (0x135, "BTN_Z"),
                (0x136, "BTN_TL"),
                (0x137, "BTN_TR"),
                (0x138, "BTN_TL2"),
                (0x139, "BTN_TR2"),
                (0x13a, "BTN_SELECT"),
                (0x13b, "BTN_START"),
                (0x13c, "BTN_MODE/Home"),
                (0x13d, "BTN_THUMBL"),
                (0x13e, "BTN_THUMBR"),
            ];
            KEY.iter()
                .find(|(c, _)| *c == code)
                .map(|(_, n)| (*n).to_string())
                .unwrap_or_else(|| format!("KEY_{code}"))
        }
        EV_ABS => {
            const ABS: &[(u16, &str)] = &[
                (0, "ABS_X"),
                (1, "ABS_Y"),
                (2, "ABS_Z"),
                (3, "ABS_RX"),
                (4, "ABS_RY"),
                (5, "ABS_RZ"),
                (16, "ABS_HAT0X"),
                (17, "ABS_HAT0Y"),
            ];
            ABS.iter()
                .find(|(c, _)| *c == code)
                .map(|(_, n)| (*n).to_string())
                .unwrap_or_else(|| format!("ABS_{code}"))
        }
        EV_MSC => format!("MSC_{code}"),
        _ => format!("code_{code}"),
    }
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn resolve_event_path(js_path: &str) -> io::Result<String> {
    let js_node = resolve_js_node(js_path)?;
    let num = js_node
        .strip_prefix("js")
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "bad js node"))?;
    Ok(format!("/dev/input/event{num}"))
}

fn resolve_hidraw_path(js_path: &str) -> Option<String> {
    let js_node = resolve_js_node(js_path).ok()?;
    let input_dev = std::fs::canonicalize(format!("/sys/class/input/{js_node}/device")).ok()?;
    // .../0003:VID:PID.N/input/inputM -> hid node is two levels up
    let hid_dev = input_dev.parent()?.parent()?;
    let hidraw_dir = hid_dev.join("hidraw");
    let entry = std::fs::read_dir(hidraw_dir).ok()?.next()?.ok()?;
    Some(format!("/dev/{}", entry.file_name().to_string_lossy()))
}

/// Raw joystick event logger for calibration.
pub fn log_js_events(path: Option<&str>, secs: u64) -> io::Result<()> {
    let reader = match path {
        Some(p) => PadReader::open_path(p)?,
        None => PadReader::open()?,
    };
    eprintln!("logging {} for {secs}s (press buttons / move sticks)...", reader.path);
    let start = std::time::Instant::now();
    let mut file = reader.file;
    let mut buf = [0u8; JS_EVENT_SIZE];
    while start.elapsed().as_secs() < secs {
        match file.read(&mut buf) {
            Ok(0) => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(n) if n >= JS_EVENT_SIZE => {
                let event_type = buf[6] & !JS_EVENT_INIT;
                let number = buf[7];
                let value = i16::from_le_bytes([buf[4], buf[5]]);
                let kind = match event_type {
                    JS_EVENT_BUTTON => "btn",
                    JS_EVENT_AXIS => "axis",
                    _ => "?",
                };
                println!("{kind} {number} = {value}");
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Interactive calibrate: print labeled prompts, user presses each control.
pub fn calibrate(path: Option<&str>) -> io::Result<()> {
    let reader = match path {
        Some(p) => PadReader::open_path(p)?,
        None => PadReader::open()?,
    };
    eprintln!("calibrate on {} — press each control when prompted (10s timeout each)", reader.path);
    let prompts: &[(&str, fn(&PadState) -> bool)] = &[
        ("A", |s| s.btn_a),
        ("B", |s| s.btn_b),
        ("X", |s| s.btn_x),
        ("Y", |s| s.btn_y),
        ("L", |s| s.btn_l),
        ("R", |s| s.btn_r),
        ("ZL", |s| s.btn_zl),
        ("ZR", |s| s.btn_zr),
        ("Select", |s| s.btn_select),
        ("Start", |s| s.btn_start),
        ("L3", |s| s.btn_l3),
        ("R3", |s| s.btn_r3),
        ("D-pad Up", |s| s.dpad_up),
        ("D-pad Down", |s| s.dpad_down),
        ("D-pad Left", |s| s.dpad_left),
        ("D-pad Right", |s| s.dpad_right),
    ];
    let mut file = reader.file;
    let mut buf = [0u8; JS_EVENT_SIZE];
    let mut state = PadState::default();
    for (label, _) in prompts {
        println!("\n>>> Press [{label}] ...");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while std::time::Instant::now() < deadline {
            if read_one_event(&mut file, &mut buf, &mut state) {
                println!("    raw: {}", state.last_raw);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    println!("\ncalibrate done — move left stick, then right stick (5s each)");
    for label in ["left stick", "right stick"] {
        println!("\n>>> Move [{label}] ...");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if read_one_event(&mut file, &mut buf, &mut state) {
                println!(
                    "    raw: {}  L=({:.2},{:.2}) R=({:.2},{:.2})",
                    state.last_raw, state.left_x, state.left_y, state.right_x, state.right_y
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    Ok(())
}

fn read_one_event(file: &mut File, buf: &mut [u8; JS_EVENT_SIZE], state: &mut PadState) -> bool {
    match file.read(buf) {
        Ok(n) if n >= JS_EVENT_SIZE => {
            let event_type = buf[6] & !JS_EVENT_INIT;
            let number = buf[7];
            let value = i16::from_le_bytes([buf[4], buf[5]]);
            state.apply_event(event_type, number, value)
        }
        _ => false,
    }
}
