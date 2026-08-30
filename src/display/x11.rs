use std::borrow::Cow;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::fs::File;
use std::thread;
use std::time::{Duration, Instant};

use memmap2::{MmapMut, MmapOptions};
use x11rb::CURRENT_TIME;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::image::{BitsPerPixel, Image, ImageOrder, PixelLayout, ScanlinePad};
use x11rb::protocol::Event;
use x11rb::protocol::damage::{self, ConnectionExt as DamageConnectionExt};
use x11rb::protocol::shm::{self, ConnectionExt as ShmConnectionExt};
use x11rb::protocol::xproto::{
    BUTTON_PRESS_EVENT, BUTTON_RELEASE_EVENT, ConfigureWindowAux, ConnectionExt, ImageFormat,
    InputFocus, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, MOTION_NOTIFY_EVENT, MapState, Visualid,
    Window,
};
use x11rb::protocol::xtest::ConnectionExt as XTestConnectionExt;
use x11rb::rust_connection::RustConnection;
use xkeysym::key;

use crate::protocol::modifiers;
use crate::protocol::{InputEvent, KeyEvent, MouseButton, MouseEvent, MouseKind};
use crate::renderer::Frame;

pub struct X11Display {
    connection: RustConnection,
    root: Window,
    width: u16,
    height: u16,
    pixel_layout: PixelLayout,
    keyboard_map: KeyboardMap,
    shm_capture: Option<ShmCapture>,
    damage: Option<DamageTracker>,
}

impl X11Display {
    pub fn connect(display_name: &str) -> Result<Self, X11Error> {
        let (connection, screen_number) = x11rb::connect(Some(display_name))
            .map_err(|error| X11Error::Connect(error.to_string()))?;
        let screen = connection
            .setup()
            .roots
            .get(screen_number)
            .ok_or(X11Error::MissingScreen)?;
        let root = screen.root;
        let width = screen.width_in_pixels;
        let height = screen.height_in_pixels;
        let visual = find_visual(&connection, screen_number, screen.root_visual)
            .ok_or(X11Error::MissingRootVisual)?;
        let pixel_layout = PixelLayout::from_visual_type(visual)
            .map_err(|error| X11Error::UnsupportedVisual(error.to_string()))?;
        connection
            .xtest_get_version(2, 2)
            .map_err(connection_error)?
            .reply()
            .map_err(|error| X11Error::UnsupportedXTest(error.to_string()))?;
        let keyboard_map = KeyboardMap::load(&connection)?;
        let shm_capture = ShmCapture::try_new(&connection, width, height, screen.root_depth)
            .ok()
            .flatten();
        let damage = DamageTracker::try_new(&connection, root).ok().flatten();

        Ok(Self {
            connection,
            root,
            width,
            height,
            pixel_layout,
            keyboard_map,
            shm_capture,
            damage,
        })
    }

    pub fn wait_for_application_window(&self, timeout: Duration) -> Result<Window, X11Error> {
        let deadline = Instant::now() + timeout;
        loop {
            let tree = self
                .connection
                .query_tree(self.root)
                .map_err(connection_error)?
                .reply()
                .map_err(reply_error)?;
            for &window in tree.children.iter().rev() {
                let attributes = self
                    .connection
                    .get_window_attributes(window)
                    .map_err(connection_error)?
                    .reply()
                    .map_err(reply_error)?;
                if attributes.map_state == MapState::VIEWABLE {
                    return Ok(window);
                }
            }

            if Instant::now() >= deadline {
                return Err(X11Error::WindowTimeout(timeout));
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    pub fn fill_screen(&self, window: Window) -> Result<(), X11Error> {
        let values = ConfigureWindowAux::new()
            .x(0)
            .y(0)
            .width(u32::from(self.width))
            .height(u32::from(self.height))
            .border_width(0);
        self.connection
            .configure_window(window, &values)
            .map_err(connection_error)?
            .check()
            .map_err(reply_error)?;
        self.connection
            .set_input_focus(InputFocus::PARENT, window, CURRENT_TIME)
            .map_err(connection_error)?
            .check()
            .map_err(reply_error)?;
        self.connection.flush().map_err(connection_error)
    }

    pub fn capture(&mut self) -> Result<Frame, X11Error> {
        if let Some(shm_capture) = &mut self.shm_capture {
            let reply = self
                .connection
                .shm_get_image(
                    self.root,
                    0,
                    0,
                    self.width,
                    self.height,
                    !0,
                    ImageFormat::Z_PIXMAP.into(),
                    shm_capture.segment,
                    0,
                )
                .map_err(connection_error)?
                .reply()
                .map_err(reply_error)?;
            if reply.size as usize > shm_capture.mapping.len() {
                return Err(X11Error::InvalidSharedImageSize {
                    expected: shm_capture.mapping.len(),
                    actual: reply.size as usize,
                });
            }
            let image = Image::new(
                self.width,
                self.height,
                shm_capture.scanline_pad,
                reply.depth,
                shm_capture.bits_per_pixel,
                shm_capture.byte_order,
                Cow::Borrowed(&shm_capture.mapping[..]),
            )
            .map_err(|error| X11Error::Capture(error.to_string()))?;
            return frame_from_image(&image, self.width, self.height, self.pixel_layout);
        }

        let (image, visual) =
            Image::get(&self.connection, self.root, 0, 0, self.width, self.height)
                .map_err(|error| X11Error::Capture(error.to_string()))?;

        let pixel_layout = if visual == 0 {
            self.pixel_layout
        } else {
            let visual = find_visual_by_id(&self.connection, visual)
                .ok_or(X11Error::MissingCaptureVisual(visual))?;
            PixelLayout::from_visual_type(visual)
                .map_err(|error| X11Error::UnsupportedVisual(error.to_string()))?
        };

        frame_from_image(&image, self.width, self.height, pixel_layout)
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn capture_method(&self) -> &'static str {
        if self.shm_capture.is_some() {
            "MIT-SHM"
        } else {
            "GetImage"
        }
    }

    pub fn frame_pending(&mut self) -> Result<bool, X11Error> {
        match &mut self.damage {
            Some(damage) => damage.take(&self.connection),
            None => Ok(true),
        }
    }

    #[cfg(test)]
    pub(crate) fn pointer_position(&self) -> Result<(i16, i16), X11Error> {
        let reply = self
            .connection
            .query_pointer(self.root)
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        Ok((reply.root_x, reply.root_y))
    }

    pub fn inject(&self, event: &InputEvent) -> Result<(), X11Error> {
        match event {
            InputEvent::Key(event) => self.inject_key(event)?,
            InputEvent::Mouse(event) => self.inject_mouse(event)?,
            InputEvent::Resize { .. } => {}
        }
        self.connection.flush().map_err(connection_error)
    }

    fn inject_key(&self, event: &KeyEvent) -> Result<(), X11Error> {
        let keycode = self
            .keyboard_map
            .keycode_for(event.code)
            .ok_or(X11Error::UnmappedKeysym(event.code))?;
        let modifier_keycodes = self.modifier_keycodes(event.modifiers)?;

        if event.pressed {
            for &modifier in &modifier_keycodes {
                self.fake_key(modifier, true)?;
            }
            self.fake_key(keycode, true)
        } else {
            self.fake_key(keycode, false)?;
            for &modifier in modifier_keycodes.iter().rev() {
                self.fake_key(modifier, false)?;
            }
            Ok(())
        }
    }

    fn modifier_keycodes(&self, value: u8) -> Result<Vec<u8>, X11Error> {
        let mut result = Vec::with_capacity(4);
        let requested = [
            (modifiers::SHIFT, key::Shift_L),
            (modifiers::CONTROL, key::Control_L),
            (modifiers::ALT, key::Alt_L),
            (modifiers::SUPER, key::Super_L),
        ];
        for (flag, keysym) in requested {
            if value & flag != 0 {
                result.push(
                    self.keyboard_map
                        .keycode_for(keysym)
                        .ok_or(X11Error::UnmappedKeysym(keysym))?,
                );
            }
        }
        Ok(result)
    }

    fn fake_key(&self, keycode: u8, pressed: bool) -> Result<(), X11Error> {
        self.connection
            .xtest_fake_input(
                if pressed {
                    KEY_PRESS_EVENT
                } else {
                    KEY_RELEASE_EVENT
                },
                keycode,
                CURRENT_TIME,
                self.root,
                0,
                0,
                0,
            )
            .map_err(connection_error)?
            .check()
            .map_err(reply_error)
    }

    fn inject_mouse(&self, event: &MouseEvent) -> Result<(), X11Error> {
        let x = i16::try_from(event.x.min(u32::from(self.width.saturating_sub(1))))
            .map_err(|_| X11Error::InvalidPointerCoordinate)?;
        let y = i16::try_from(event.y.min(u32::from(self.height.saturating_sub(1))))
            .map_err(|_| X11Error::InvalidPointerCoordinate)?;
        if event.kind == MouseKind::Move {
            self.connection
                .xtest_fake_input(MOTION_NOTIFY_EVENT, 0, CURRENT_TIME, self.root, x, y, 0)
                .map_err(connection_error)?
                .check()
                .map_err(reply_error)?;
        }
        if let Some(button) = event.button.filter(|_| event.kind != MouseKind::Move) {
            let modifier_keycodes = self.modifier_keycodes(event.modifiers)?;
            if event.kind == MouseKind::Press {
                for &modifier in &modifier_keycodes {
                    self.fake_key(modifier, true)?;
                }
            }
            self.connection
                .xtest_fake_input(
                    if event.kind == MouseKind::Press {
                        BUTTON_PRESS_EVENT
                    } else {
                        BUTTON_RELEASE_EVENT
                    },
                    mouse_button_number(button),
                    CURRENT_TIME,
                    self.root,
                    x,
                    y,
                    0,
                )
                .map_err(connection_error)?
                .check()
                .map_err(reply_error)?;
            if event.kind == MouseKind::Release {
                for &modifier in modifier_keycodes.iter().rev() {
                    self.fake_key(modifier, false)?;
                }
            }
        }
        Ok(())
    }
}

impl Drop for X11Display {
    fn drop(&mut self) {
        if let Some(damage) = &self.damage {
            if let Ok(cookie) = self.connection.damage_destroy(damage.id) {
                let _ = cookie.check();
            }
        }
        if let Some(shm_capture) = &self.shm_capture {
            if let Ok(cookie) = self.connection.shm_detach(shm_capture.segment) {
                let _ = cookie.check();
            }
        }
    }
}

struct DamageTracker {
    id: damage::Damage,
    first_frame: bool,
}

impl DamageTracker {
    fn try_new(connection: &RustConnection, root: Window) -> Result<Option<Self>, X11Error> {
        if connection
            .extension_information(damage::X11_EXTENSION_NAME)
            .map_err(connection_error)?
            .is_none()
        {
            return Ok(None);
        }
        connection
            .damage_query_version(1, 1)
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        let id = connection
            .generate_id()
            .map_err(|error| X11Error::Protocol(error.to_string()))?;
        connection
            .damage_create(id, root, damage::ReportLevel::NON_EMPTY)
            .map_err(connection_error)?
            .check()
            .map_err(reply_error)?;
        connection.flush().map_err(connection_error)?;
        Ok(Some(Self {
            id,
            first_frame: true,
        }))
    }

    fn take(&mut self, connection: &RustConnection) -> Result<bool, X11Error> {
        let mut damaged = std::mem::take(&mut self.first_frame);
        while let Some(event) = connection.poll_for_event().map_err(connection_error)? {
            if matches!(event, Event::DamageNotify(ref notify) if notify.damage == self.id) {
                damaged = true;
            }
        }
        if damaged {
            connection
                .damage_subtract(self.id, x11rb::NONE, x11rb::NONE)
                .map_err(connection_error)?
                .check()
                .map_err(reply_error)?;
            connection.flush().map_err(connection_error)?;
        }
        Ok(damaged)
    }
}

struct ShmCapture {
    segment: shm::Seg,
    mapping: MmapMut,
    scanline_pad: ScanlinePad,
    bits_per_pixel: BitsPerPixel,
    byte_order: ImageOrder,
}

impl ShmCapture {
    fn try_new(
        connection: &RustConnection,
        width: u16,
        height: u16,
        depth: u8,
    ) -> Result<Option<Self>, X11Error> {
        if connection
            .extension_information(shm::X11_EXTENSION_NAME)
            .map_err(connection_error)?
            .is_none()
        {
            return Ok(None);
        }
        let version = connection
            .shm_query_version()
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        if version.major_version < 1 || (version.major_version == 1 && version.minor_version < 2) {
            return Ok(None);
        }

        let template = Image::allocate_native(width, height, depth, connection.setup())
            .map_err(|error| X11Error::Capture(error.to_string()))?;
        let byte_length = template.data().len();
        let segment = connection
            .generate_id()
            .map_err(|error| X11Error::Protocol(error.to_string()))?;
        let reply = connection
            .shm_create_segment(
                segment,
                u32::try_from(byte_length).map_err(|_| X11Error::SharedImageTooLarge)?,
                false,
            )
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        let file: File = reply.shm_fd.into();
        // SAFETY: The X server created this file descriptor with exactly byte_length
        // bytes for this client, and the mapping does not outlive the owned X11 display.
        let mapping = unsafe { MmapOptions::new().len(byte_length).map_mut(&file) }
            .map_err(|error| X11Error::SharedMemoryMap(error.to_string()))?;

        Ok(Some(Self {
            segment,
            mapping,
            scanline_pad: template.scanline_pad(),
            bits_per_pixel: template.bits_per_pixel(),
            byte_order: template.byte_order(),
        }))
    }
}

fn frame_from_image(
    image: &Image<'_>,
    width: u16,
    height: u16,
    pixel_layout: PixelLayout,
) -> Result<Frame, X11Error> {
    let capacity = Frame::rgb_buffer_len(u32::from(width), u32::from(height))
        .map_err(|error| X11Error::Capture(error.to_string()))?;
    let mut pixels = Vec::with_capacity(capacity);
    for y in 0..height {
        for x in 0..width {
            let (red, green, blue) = pixel_layout.decode(image.get_pixel(x, y));
            pixels.extend_from_slice(&[(red >> 8) as u8, (green >> 8) as u8, (blue >> 8) as u8]);
        }
    }
    Frame::new_rgb(u32::from(width), u32::from(height), pixels)
        .map_err(|error| X11Error::Capture(error.to_string()))
}

struct KeyboardMap {
    minimum_keycode: u8,
    keysyms_per_keycode: usize,
    keysyms: Vec<u32>,
}

impl KeyboardMap {
    fn load(connection: &RustConnection) -> Result<Self, X11Error> {
        let setup = connection.setup();
        let minimum_keycode = setup.min_keycode;
        let count = setup
            .max_keycode
            .checked_sub(minimum_keycode)
            .and_then(|value| value.checked_add(1))
            .ok_or(X11Error::InvalidKeyboardMap)?;
        let reply = connection
            .get_keyboard_mapping(minimum_keycode, count)
            .map_err(connection_error)?
            .reply()
            .map_err(reply_error)?;
        if reply.keysyms_per_keycode == 0 {
            return Err(X11Error::InvalidKeyboardMap);
        }
        Ok(Self {
            minimum_keycode,
            keysyms_per_keycode: usize::from(reply.keysyms_per_keycode),
            keysyms: reply.keysyms,
        })
    }

    fn keycode_for(&self, keysym: u32) -> Option<u8> {
        self.keysyms
            .chunks_exact(self.keysyms_per_keycode)
            .position(|symbols| symbols.contains(&keysym))
            .and_then(|offset| self.minimum_keycode.checked_add(offset as u8))
    }
}

fn mouse_button_number(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
        MouseButton::WheelUp => 4,
        MouseButton::WheelDown => 5,
    }
}

fn find_visual(
    connection: &RustConnection,
    screen_number: usize,
    visual_id: Visualid,
) -> Option<x11rb::protocol::xproto::Visualtype> {
    let screen = connection.setup().roots.get(screen_number)?;
    screen
        .allowed_depths
        .iter()
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual_id == visual_id)
        .copied()
}

fn find_visual_by_id(
    connection: &RustConnection,
    visual_id: Visualid,
) -> Option<x11rb::protocol::xproto::Visualtype> {
    connection
        .setup()
        .roots
        .iter()
        .flat_map(|screen| screen.allowed_depths.iter())
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.visual_id == visual_id)
        .copied()
}

fn connection_error(error: x11rb::errors::ConnectionError) -> X11Error {
    X11Error::Protocol(error.to_string())
}

fn reply_error(error: x11rb::errors::ReplyError) -> X11Error {
    X11Error::Protocol(error.to_string())
}

#[derive(Debug)]
pub enum X11Error {
    Connect(String),
    MissingScreen,
    MissingRootVisual,
    MissingCaptureVisual(Visualid),
    UnsupportedVisual(String),
    UnsupportedXTest(String),
    InvalidKeyboardMap,
    UnmappedKeysym(u32),
    InvalidPointerCoordinate,
    SharedImageTooLarge,
    SharedMemoryMap(String),
    InvalidSharedImageSize { expected: usize, actual: usize },
    Protocol(String),
    Capture(String),
    WindowTimeout(Duration),
}

impl Display for X11Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(message) => write!(formatter, "could not connect to X11: {message}"),
            Self::MissingScreen => write!(formatter, "X11 did not report the requested screen"),
            Self::MissingRootVisual => write!(formatter, "X11 root visual was not found"),
            Self::MissingCaptureVisual(visual) => {
                write!(formatter, "X11 capture visual {visual} was not found")
            }
            Self::UnsupportedVisual(message) => {
                write!(formatter, "unsupported X11 visual: {message}")
            }
            Self::UnsupportedXTest(message) => {
                write!(formatter, "XTEST extension is unavailable: {message}")
            }
            Self::InvalidKeyboardMap => write!(formatter, "X11 returned an invalid keyboard map"),
            Self::UnmappedKeysym(keysym) => {
                write!(
                    formatter,
                    "keysym 0x{keysym:x} is not present in the X11 keymap"
                )
            }
            Self::InvalidPointerCoordinate => {
                write!(formatter, "pointer coordinate exceeds X11 range")
            }
            Self::SharedImageTooLarge => write!(formatter, "shared X11 image is too large"),
            Self::SharedMemoryMap(message) => {
                write!(formatter, "could not map X11 shared memory: {message}")
            }
            Self::InvalidSharedImageSize { expected, actual } => write!(
                formatter,
                "X11 shared image size is invalid: capacity {expected}, reply {actual}"
            ),
            Self::Protocol(message) => write!(formatter, "X11 protocol error: {message}"),
            Self::Capture(message) => write!(formatter, "X11 frame capture failed: {message}"),
            Self::WindowTimeout(timeout) => write!(
                formatter,
                "application did not map a window within {:.1}s",
                timeout.as_secs_f32()
            ),
        }
    }
}

impl Error for X11Error {}
