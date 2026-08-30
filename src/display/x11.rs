use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::thread;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::image::{Image, PixelLayout};
use x11rb::protocol::xproto::{ConfigureWindowAux, ConnectionExt, MapState, Visualid, Window};
use x11rb::rust_connection::RustConnection;

use crate::renderer::Frame;

pub struct X11Display {
    connection: RustConnection,
    root: Window,
    width: u16,
    height: u16,
    pixel_layout: PixelLayout,
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

        Ok(Self {
            connection,
            root,
            width,
            height,
            pixel_layout,
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
        self.connection.flush().map_err(connection_error)
    }

    pub fn capture(&self) -> Result<Frame, X11Error> {
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

        let mut pixels = Vec::with_capacity(usize::from(self.width) * usize::from(self.height) * 3);
        for y in 0..self.height {
            for x in 0..self.width {
                let (red, green, blue) = pixel_layout.decode(image.get_pixel(x, y));
                pixels.extend_from_slice(&[
                    (red >> 8) as u8,
                    (green >> 8) as u8,
                    (blue >> 8) as u8,
                ]);
            }
        }
        Frame::new_rgb(u32::from(self.width), u32::from(self.height), pixels)
            .map_err(|error| X11Error::Capture(error.to_string()))
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
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
