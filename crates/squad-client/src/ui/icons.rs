use iced::{
    advanced::{
        graphics::geometry::Renderer as GeometryRenderer,
        layout, mouse,
        renderer::{self, Renderer as CoreRenderer},
        widget::Tree,
        Widget,
    },
    widget::canvas::{path::Arc, Fill, Frame, LineCap, LineJoin, Path, Stroke},
    Color, Element, Length, Point, Radians, Rectangle, Size, Theme, Vector,
};
use std::f32::consts::{FRAC_PI_2, PI};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconKind {
    Mic,
    MicOff,
    Headset,
    HeadsetOff,
    Call,
    CallEnd,
    ScreenShare,
    StopScreenShare,
    Settings,
    Key,
    ContentCopy,
    ContentPaste,
    Check,
    Refresh,
    PersonAdd,
    Add,
    Delete,
    Bookmark,
    BookmarkAdded,
    Visibility,
    VisibilityOff,
    PlayArrow,
    Stop,
    VolumeUp,
    AttachFile,
    Send,
    Download,
    FolderOpen,
    Expand,
    Collapse,
    Fullscreen,
    FullscreenExit,
    Close,
}

impl IconKind {
    #[cfg(test)]
    pub const ALL: [IconKind; 33] = [
        IconKind::Mic,
        IconKind::MicOff,
        IconKind::Headset,
        IconKind::HeadsetOff,
        IconKind::Call,
        IconKind::CallEnd,
        IconKind::ScreenShare,
        IconKind::StopScreenShare,
        IconKind::Settings,
        IconKind::Key,
        IconKind::ContentCopy,
        IconKind::ContentPaste,
        IconKind::Check,
        IconKind::Refresh,
        IconKind::PersonAdd,
        IconKind::Add,
        IconKind::Delete,
        IconKind::Bookmark,
        IconKind::BookmarkAdded,
        IconKind::Visibility,
        IconKind::VisibilityOff,
        IconKind::PlayArrow,
        IconKind::Stop,
        IconKind::VolumeUp,
        IconKind::AttachFile,
        IconKind::Send,
        IconKind::Download,
        IconKind::FolderOpen,
        IconKind::Expand,
        IconKind::Collapse,
        IconKind::Fullscreen,
        IconKind::FullscreenExit,
        IconKind::Close,
    ];
}

pub struct Icon {
    kind: IconKind,
    size: f32,
    color: Option<Color>,
}

impl Icon {
    pub fn new(kind: IconKind, size: f32) -> Self {
        Self {
            kind,
            size,
            color: None,
        }
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

pub fn icon<'a, Message: 'a>(kind: IconKind, size: f32) -> Element<'a, Message> {
    Element::new(Icon::new(kind, size))
}

pub fn icon_colored<'a, Message: 'a>(
    kind: IconKind,
    size: f32,
    color: Color,
) -> Element<'a, Message> {
    Element::new(Icon::new(kind, size).with_color(color))
}

impl<Message> Widget<Message, Theme, iced::Renderer> for Icon {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: layout::Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let color = self.color.unwrap_or(style.text_color);
        let kind = self.kind;
        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            let mut frame = Frame::new(renderer, bounds.size());
            let scale = (bounds.width.min(bounds.height) / 24.0).max(0.01);
            frame.scale(scale);
            draw_icon(&mut frame, kind, color);
            renderer.draw_geometry(frame.into_geometry());
        });
    }
}

impl<'a, Message: 'a> From<Icon> for Element<'a, Message> {
    fn from(widget: Icon) -> Self {
        Element::new(widget)
    }
}

pub struct AnimatedCheck {
    size: f32,
    draw_progress: f32,
    pop_pulse: f32,
    color: Option<Color>,
}

impl AnimatedCheck {
    pub fn new(size: f32, draw_progress: f32, pop_pulse: f32) -> Self {
        Self {
            size,
            draw_progress: draw_progress.clamp(0.0, 1.0),
            pop_pulse: pop_pulse.clamp(0.0, 1.0),
            color: None,
        }
    }
}

pub fn icon_check_animated<'a, Message: 'a>(
    size: f32,
    draw_progress: f32,
    pop_pulse: f32,
) -> Element<'a, Message> {
    Element::new(AnimatedCheck::new(size, draw_progress, pop_pulse))
}

impl<Message> Widget<Message, Theme, iced::Renderer> for AnimatedCheck {
    fn size(&self) -> Size<Length> {
        Size::new(Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn layout(
        &self,
        _tree: &mut Tree,
        _renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        layout::atomic(limits, Length::Fixed(self.size), Length::Fixed(self.size))
    }

    fn draw(
        &self,
        _tree: &Tree,
        renderer: &mut iced::Renderer,
        _theme: &Theme,
        style: &renderer::Style,
        layout: layout::Layout<'_>,
        _cursor: mouse::Cursor,
        _viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        let color = self.color.unwrap_or(style.text_color);
        let draw_progress = self.draw_progress;
        let pop_pulse = self.pop_pulse;
        renderer.with_translation(Vector::new(bounds.x, bounds.y), |renderer| {
            let mut frame = Frame::new(renderer, bounds.size());
            let scale = (bounds.width.min(bounds.height) / 24.0).max(0.01);
            frame.scale(scale);
            draw_animated_check(&mut frame, color, draw_progress, pop_pulse);
            renderer.draw_geometry(frame.into_geometry());
        });
    }
}

impl<'a, Message: 'a> From<AnimatedCheck> for Element<'a, Message> {
    fn from(widget: AnimatedCheck) -> Self {
        Element::new(widget)
    }
}

fn round_stroke(color: Color, width: f32) -> Stroke<'static> {
    Stroke::default()
        .with_color(color)
        .with_width(width)
        .with_line_cap(LineCap::Round)
        .with_line_join(LineJoin::Round)
}

fn pt(x: f32, y: f32) -> Point {
    Point::new(x, y)
}

fn lerp_pt(a: Point, b: Point, t: f32) -> Point {
    Point::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

fn draw_animated_check(frame: &mut Frame, color: Color, draw_progress: f32, pop_pulse: f32) {
    let t = draw_progress.clamp(0.0, 1.0);
    if t <= 0.01 {
        return;
    }
    let scale = 0.78 + 0.22 * t + 0.10 * pop_pulse;
    let map_pt =
        |x: f32, y: f32| -> Point { pt(12.0 + (x - 12.0) * scale, 12.0 + (y - 12.0) * scale) };
    let p0 = map_pt(4.8, 12.6);
    let p1 = map_pt(9.8, 17.6);
    let p2 = map_pt(19.4, 7.2);
    let stroke = round_stroke(color, 2.4);

    let path = Path::new(|b| {
        b.move_to(p0);
        if t <= 0.35 {
            b.line_to(lerp_pt(p0, p1, t / 0.35));
        } else {
            b.line_to(p1);
            b.line_to(lerp_pt(p1, p2, (t - 0.35) / 0.65));
        }
    });
    frame.stroke(&path, stroke);
}

fn draw_icon(frame: &mut Frame, kind: IconKind, color: Color) {
    let stroke = round_stroke(color, 2.0);
    let thin_stroke = round_stroke(color, 1.75);
    let fill = Fill::from(color);

    match kind {
        IconKind::Mic => {
            let capsule = Path::new(|b| {
                b.rounded_rectangle(pt(9.0, 3.0), Size::new(6.0, 11.0), 3.0.into());
            });
            frame.stroke(&capsule, stroke);
            let cradle = Path::new(|b| {
                b.arc(Arc {
                    center: pt(12.0, 11.5),
                    radius: 5.5,
                    start_angle: Radians(0.0),
                    end_angle: Radians(PI),
                });
                b.move_to(pt(12.0, 17.0));
                b.line_to(pt(12.0, 20.5));
                b.move_to(pt(8.5, 20.5));
                b.line_to(pt(15.5, 20.5));
            });
            frame.stroke(&cradle, stroke);
        }
        IconKind::MicOff => {
            let capsule = Path::new(|b| {
                b.rounded_rectangle(pt(9.0, 3.0), Size::new(6.0, 11.0), 3.0.into());
            });
            frame.stroke(&capsule, stroke);
            let base = Path::new(|b| {
                b.arc(Arc {
                    center: pt(12.0, 11.5),
                    radius: 5.5,
                    start_angle: Radians(0.18),
                    end_angle: Radians(PI - 0.18),
                });
                b.move_to(pt(12.0, 17.0));
                b.line_to(pt(12.0, 20.5));
                b.move_to(pt(8.5, 20.5));
                b.line_to(pt(15.5, 20.5));
            });
            frame.stroke(&base, stroke);
            let slash = Path::line(pt(4.0, 4.0), pt(20.0, 20.0));
            frame.stroke(&slash, round_stroke(color, 2.2));
        }
        IconKind::Headset => {
            let band = Path::new(|b| {
                b.arc(Arc {
                    center: pt(12.0, 12.5),
                    radius: 7.5,
                    start_angle: Radians(PI),
                    end_angle: Radians(2.0 * PI),
                });
            });
            frame.stroke(&band, stroke);
            let cups = Path::new(|b| {
                b.rounded_rectangle(pt(4.0, 12.0), Size::new(3.8, 7.0), 1.6.into());
                b.rounded_rectangle(pt(16.2, 12.0), Size::new(3.8, 7.0), 1.6.into());
            });
            frame.stroke(&cups, stroke);
        }
        IconKind::HeadsetOff => {
            let band = Path::new(|b| {
                b.arc(Arc {
                    center: pt(12.0, 12.5),
                    radius: 7.5,
                    start_angle: Radians(PI + 0.15),
                    end_angle: Radians(2.0 * PI - 0.15),
                });
            });
            frame.stroke(&band, stroke);
            let cups = Path::new(|b| {
                b.rounded_rectangle(pt(4.0, 12.0), Size::new(3.8, 7.0), 1.6.into());
                b.rounded_rectangle(pt(16.2, 12.0), Size::new(3.8, 7.0), 1.6.into());
            });
            frame.stroke(&cups, stroke);
            let slash = Path::line(pt(4.0, 4.0), pt(20.0, 20.0));
            frame.stroke(&slash, round_stroke(color, 2.2));
        }
        IconKind::Call => {
            let handset = Path::new(|b| {
                b.move_to(pt(6.2, 4.2));
                b.line_to(pt(9.3, 4.2));
                b.line_to(pt(10.5, 8.2));
                b.line_to(pt(8.3, 10.0));
                b.quadratic_curve_to(pt(10.2, 13.8), pt(14.0, 15.7));
                b.line_to(pt(15.8, 13.5));
                b.line_to(pt(19.8, 14.7));
                b.line_to(pt(19.8, 17.8));
                b.quadratic_curve_to(pt(19.4, 19.8), pt(16.8, 19.8));
                b.quadratic_curve_to(pt(6.5, 18.2), pt(4.2, 7.2));
                b.quadratic_curve_to(pt(4.2, 4.6), pt(6.2, 4.2));
                b.close();
            });
            frame.stroke(&handset, stroke);
        }
        IconKind::CallEnd => {
            let arc = Path::new(|b| {
                b.move_to(pt(3.0, 14.5));
                b.quadratic_curve_to(pt(12.0, 7.5), pt(21.0, 14.5));
                b.line_to(pt(19.0, 17.2));
                b.line_to(pt(15.5, 16.0));
                b.line_to(pt(15.2, 13.4));
                b.quadratic_curve_to(pt(12.0, 12.2), pt(8.8, 13.4));
                b.line_to(pt(8.5, 16.0));
                b.line_to(pt(5.0, 17.2));
                b.close();
            });
            frame.fill(&arc, fill);
        }
        IconKind::ScreenShare => {
            let screen = Path::new(|b| {
                b.rounded_rectangle(pt(3.0, 4.5), Size::new(18.0, 12.5), 2.0.into());
                b.move_to(pt(8.0, 20.0));
                b.line_to(pt(16.0, 20.0));
                b.move_to(pt(12.0, 17.0));
                b.line_to(pt(12.0, 20.0));
            });
            frame.stroke(&screen, stroke);
            let arrow = Path::new(|b| {
                b.move_to(pt(12.0, 13.5));
                b.line_to(pt(12.0, 8.0));
                b.move_to(pt(9.2, 10.5));
                b.line_to(pt(12.0, 7.7));
                b.line_to(pt(14.8, 10.5));
            });
            frame.stroke(&arrow, stroke);
        }
        IconKind::StopScreenShare => {
            let screen = Path::new(|b| {
                b.rounded_rectangle(pt(3.0, 4.5), Size::new(18.0, 12.5), 2.0.into());
                b.move_to(pt(8.0, 20.0));
                b.line_to(pt(16.0, 20.0));
                b.move_to(pt(12.0, 17.0));
                b.line_to(pt(12.0, 20.0));
            });
            frame.stroke(&screen, stroke);
            let stop_sq = Path::new(|b| {
                b.rounded_rectangle(pt(9.2, 8.0), Size::new(5.6, 5.6), 1.0.into());
            });
            frame.fill(&stop_sq, fill);
        }
        IconKind::Settings => {
            let inner = Path::circle(pt(12.0, 12.0), 3.1);
            frame.stroke(&inner, stroke);
            let outer_ring = Path::circle(pt(12.0, 12.0), 6.5);
            frame.stroke(&outer_ring, stroke);
            let teeth = Path::new(|b| {
                for i in 0..8 {
                    let angle = (i as f32) * (PI / 4.0);
                    let (sin_a, cos_a) = angle.sin_cos();
                    b.move_to(pt(12.0 + 6.7 * cos_a, 12.0 + 6.7 * sin_a));
                    b.line_to(pt(12.0 + 9.2 * cos_a, 12.0 + 9.2 * sin_a));
                }
            });
            frame.stroke(&teeth, round_stroke(color, 2.3));
        }
        IconKind::Key => {
            let head = Path::circle(pt(7.5, 12.0), 3.8);
            frame.stroke(&head, stroke);
            let shaft = Path::new(|b| {
                b.move_to(pt(11.3, 12.0));
                b.line_to(pt(20.5, 12.0));
                b.move_to(pt(17.0, 12.0));
                b.line_to(pt(17.0, 15.8));
                b.move_to(pt(20.0, 12.0));
                b.line_to(pt(20.0, 15.0));
            });
            frame.stroke(&shaft, stroke);
        }
        IconKind::ContentCopy => {
            let back = Path::new(|b| {
                b.move_to(pt(15.5, 4.0));
                b.line_to(pt(6.5, 4.0));
                b.arc_to(pt(5.0, 4.0), pt(5.0, 5.5), 1.5);
                b.line_to(pt(5.0, 15.5));
            });
            frame.stroke(&back, thin_stroke);
            let front = Path::new(|b| {
                b.rounded_rectangle(pt(8.5, 7.5), Size::new(10.5, 12.5), 1.8.into());
            });
            frame.stroke(&front, stroke);
        }
        IconKind::ContentPaste => {
            let board = Path::new(|b| {
                b.rounded_rectangle(pt(5.0, 5.0), Size::new(14.0, 15.5), 2.0.into());
                b.rounded_rectangle(pt(9.0, 3.0), Size::new(6.0, 3.5), 1.0.into());
                b.move_to(pt(8.5, 11.5));
                b.line_to(pt(15.5, 11.5));
                b.move_to(pt(8.5, 15.2));
                b.line_to(pt(13.5, 15.2));
            });
            frame.stroke(&board, stroke);
        }
        IconKind::Check => {
            let check = Path::new(|b| {
                b.move_to(pt(4.8, 12.6));
                b.line_to(pt(9.8, 17.6));
                b.line_to(pt(19.4, 7.2));
            });
            frame.stroke(&check, round_stroke(color, 2.4));
        }
        IconKind::Refresh => {
            let arc = Path::new(|b| {
                b.arc(Arc {
                    center: pt(12.0, 12.0),
                    radius: 7.0,
                    start_angle: Radians(-FRAC_PI_2 + 0.45),
                    end_angle: Radians(1.5 * PI - 0.15),
                });
            });
            frame.stroke(&arc, stroke);
            let head = Path::new(|b| {
                b.move_to(pt(15.5, 4.2));
                b.line_to(pt(17.5, 8.2));
                b.line_to(pt(13.2, 8.8));
            });
            frame.stroke(&head, stroke);
        }
        IconKind::PersonAdd => {
            let head = Path::circle(pt(9.5, 7.8), 3.3);
            frame.stroke(&head, stroke);
            let shoulders = Path::new(|b| {
                b.arc(Arc {
                    center: pt(9.5, 19.5),
                    radius: 5.5,
                    start_angle: Radians(PI + 0.25),
                    end_angle: Radians(2.0 * PI - 0.25),
                });
            });
            frame.stroke(&shoulders, stroke);
            let plus = Path::new(|b| {
                b.move_to(pt(18.0, 9.5));
                b.line_to(pt(18.0, 15.5));
                b.move_to(pt(15.0, 12.5));
                b.line_to(pt(21.0, 12.5));
            });
            frame.stroke(&plus, stroke);
        }
        IconKind::Add => {
            let plus = Path::new(|b| {
                b.move_to(pt(12.0, 5.0));
                b.line_to(pt(12.0, 19.0));
                b.move_to(pt(5.0, 12.0));
                b.line_to(pt(19.0, 12.0));
            });
            frame.stroke(&plus, round_stroke(color, 2.3));
        }
        IconKind::Delete => {
            let bin = Path::new(|b| {
                b.move_to(pt(5.0, 6.8));
                b.line_to(pt(19.0, 6.8));
                b.move_to(pt(9.5, 6.8));
                b.line_to(pt(9.5, 4.3));
                b.line_to(pt(14.5, 4.3));
                b.line_to(pt(14.5, 6.8));
                b.move_to(pt(7.0, 6.8));
                b.line_to(pt(7.8, 19.5));
                b.line_to(pt(16.2, 19.5));
                b.line_to(pt(17.0, 6.8));
                b.move_to(pt(10.3, 10.2));
                b.line_to(pt(10.3, 16.2));
                b.move_to(pt(13.7, 10.2));
                b.line_to(pt(13.7, 16.2));
            });
            frame.stroke(&bin, stroke);
        }
        IconKind::Bookmark => {
            let ribbon = Path::new(|b| {
                b.move_to(pt(6.5, 4.2));
                b.line_to(pt(17.5, 4.2));
                b.line_to(pt(17.5, 19.8));
                b.line_to(pt(12.0, 15.8));
                b.line_to(pt(6.5, 19.8));
                b.close();
            });
            frame.stroke(&ribbon, stroke);
        }
        IconKind::BookmarkAdded => {
            let ribbon = Path::new(|b| {
                b.move_to(pt(6.5, 4.2));
                b.line_to(pt(17.5, 4.2));
                b.line_to(pt(17.5, 19.8));
                b.line_to(pt(12.0, 15.8));
                b.line_to(pt(6.5, 19.8));
                b.close();
            });
            frame.stroke(&ribbon, stroke);
            let check = Path::new(|b| {
                b.move_to(pt(9.2, 10.2));
                b.line_to(pt(11.3, 12.4));
                b.line_to(pt(15.2, 8.2));
            });
            frame.stroke(&check, thin_stroke);
        }
        IconKind::Visibility => {
            let eye = Path::new(|b| {
                b.move_to(pt(2.5, 12.0));
                b.quadratic_curve_to(pt(12.0, 4.5), pt(21.5, 12.0));
                b.quadratic_curve_to(pt(12.0, 19.5), pt(2.5, 12.0));
                b.close();
            });
            frame.stroke(&eye, stroke);
            let pupil = Path::circle(pt(12.0, 12.0), 2.8);
            frame.fill(&pupil, fill);
        }
        IconKind::VisibilityOff => {
            let eye = Path::new(|b| {
                b.move_to(pt(2.5, 12.0));
                b.quadratic_curve_to(pt(12.0, 4.5), pt(21.5, 12.0));
                b.quadratic_curve_to(pt(12.0, 19.5), pt(2.5, 12.0));
                b.close();
            });
            frame.stroke(&eye, stroke);
            let pupil = Path::circle(pt(12.0, 12.0), 2.2);
            frame.stroke(&pupil, thin_stroke);
            let slash = Path::line(pt(4.0, 4.0), pt(20.0, 20.0));
            frame.stroke(&slash, round_stroke(color, 2.2));
        }
        IconKind::PlayArrow => {
            let tri = Path::new(|b| {
                b.move_to(pt(7.2, 5.2));
                b.line_to(pt(18.8, 12.0));
                b.line_to(pt(7.2, 18.8));
                b.close();
            });
            frame.fill(&tri, fill);
        }
        IconKind::Stop => {
            let sq = Path::new(|b| {
                b.rounded_rectangle(pt(6.2, 6.2), Size::new(11.6, 11.6), 1.8.into());
            });
            frame.fill(&sq, fill);
        }
        IconKind::VolumeUp => {
            let speaker = Path::new(|b| {
                b.move_to(pt(3.8, 9.5));
                b.line_to(pt(7.5, 9.5));
                b.line_to(pt(12.2, 5.5));
                b.line_to(pt(12.2, 18.5));
                b.line_to(pt(7.5, 14.5));
                b.line_to(pt(3.8, 14.5));
                b.close();
            });
            frame.fill(&speaker, fill);
            let waves = Path::new(|b| {
                b.arc(Arc {
                    center: pt(13.5, 12.0),
                    radius: 3.4,
                    start_angle: Radians(-0.75),
                    end_angle: Radians(0.75),
                });
                b.arc(Arc {
                    center: pt(13.5, 12.0),
                    radius: 6.4,
                    start_angle: Radians(-0.82),
                    end_angle: Radians(0.82),
                });
            });
            frame.stroke(&waves, thin_stroke);
        }
        IconKind::AttachFile => {
            let clip = Path::new(|b| {
                b.move_to(pt(16.5, 7.5));
                b.line_to(pt(16.5, 16.0));
                b.arc(Arc {
                    center: pt(12.0, 16.0),
                    radius: 4.5,
                    start_angle: Radians(0.0),
                    end_angle: Radians(PI),
                });
                b.line_to(pt(7.5, 7.0));
                b.arc(Arc {
                    center: pt(10.5, 7.0),
                    radius: 3.0,
                    start_angle: Radians(PI),
                    end_angle: Radians(2.0 * PI),
                });
                b.line_to(pt(13.5, 15.0));
                b.arc(Arc {
                    center: pt(12.0, 15.0),
                    radius: 1.5,
                    start_angle: Radians(0.0),
                    end_angle: Radians(PI),
                });
                b.line_to(pt(10.5, 8.2));
            });
            frame.stroke(&clip, stroke);
        }
        IconKind::Send => {
            let plane = Path::new(|b| {
                b.move_to(pt(3.5, 4.5));
                b.line_to(pt(20.8, 12.0));
                b.line_to(pt(3.5, 19.5));
                b.line_to(pt(6.6, 12.0));
                b.close();
            });
            frame.stroke(&plane, stroke);
            let stem = Path::line(pt(6.6, 12.0), pt(14.5, 12.0));
            frame.stroke(&stem, thin_stroke);
        }
        IconKind::Download => {
            let arrow = Path::new(|b| {
                b.move_to(pt(12.0, 4.2));
                b.line_to(pt(12.0, 15.0));
                b.move_to(pt(7.8, 11.2));
                b.line_to(pt(12.0, 15.4));
                b.line_to(pt(16.2, 11.2));
                b.move_to(pt(5.0, 19.2));
                b.line_to(pt(19.0, 19.2));
            });
            frame.stroke(&arrow, stroke);
        }
        IconKind::FolderOpen => {
            let folder = Path::new(|b| {
                b.move_to(pt(3.5, 6.0));
                b.line_to(pt(9.5, 6.0));
                b.line_to(pt(11.3, 8.2));
                b.line_to(pt(19.5, 8.2));
                b.line_to(pt(19.5, 11.2));
                b.move_to(pt(3.5, 6.0));
                b.line_to(pt(3.5, 18.2));
                b.line_to(pt(18.2, 18.2));
                b.line_to(pt(21.0, 11.2));
                b.line_to(pt(6.4, 11.2));
                b.line_to(pt(3.5, 18.2));
            });
            frame.stroke(&folder, stroke);
        }
        IconKind::Expand => {
            let arrows = Path::new(|b| {
                b.move_to(pt(7.0, 10.5));
                b.line_to(pt(12.0, 15.5));
                b.line_to(pt(17.0, 10.5));
            });
            frame.stroke(&arrows, round_stroke(color, 2.3));
        }
        IconKind::Collapse => {
            let arrows = Path::new(|b| {
                b.move_to(pt(7.0, 14.5));
                b.line_to(pt(12.0, 9.5));
                b.line_to(pt(17.0, 14.5));
            });
            frame.stroke(&arrows, round_stroke(color, 2.3));
        }
        IconKind::Fullscreen => {
            let corners = Path::new(|b| {
                b.move_to(pt(4.5, 9.0));
                b.line_to(pt(4.5, 4.5));
                b.line_to(pt(9.0, 4.5));
                b.move_to(pt(15.0, 4.5));
                b.line_to(pt(19.5, 4.5));
                b.line_to(pt(19.5, 9.0));
                b.move_to(pt(19.5, 15.0));
                b.line_to(pt(19.5, 19.5));
                b.line_to(pt(15.0, 19.5));
                b.move_to(pt(9.0, 19.5));
                b.line_to(pt(4.5, 19.5));
                b.line_to(pt(4.5, 15.0));
            });
            frame.stroke(&corners, stroke);
        }
        IconKind::FullscreenExit => {
            let corners = Path::new(|b| {
                b.move_to(pt(4.5, 8.8));
                b.line_to(pt(8.8, 8.8));
                b.line_to(pt(8.8, 4.5));
                b.move_to(pt(19.5, 8.8));
                b.line_to(pt(15.2, 8.8));
                b.line_to(pt(15.2, 4.5));
                b.move_to(pt(19.5, 15.2));
                b.line_to(pt(15.2, 15.2));
                b.line_to(pt(15.2, 19.5));
                b.move_to(pt(4.5, 15.2));
                b.line_to(pt(8.8, 15.2));
                b.line_to(pt(8.8, 19.5));
            });
            frame.stroke(&corners, stroke);
        }
        IconKind::Close => {
            let x = Path::new(|b| {
                b.move_to(pt(6.0, 6.0));
                b.line_to(pt(18.0, 18.0));
                b.move_to(pt(18.0, 6.0));
                b.line_to(pt(6.0, 18.0));
            });
            frame.stroke(&x, round_stroke(color, 2.3));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iced::advanced::Widget;

    #[test]
    fn icon_widget_reports_fixed_layout_size() {
        for kind in IconKind::ALL {
            let widget = Icon::new(kind, 18.0);
            let size = <Icon as Widget<(), Theme, iced::Renderer>>::size(&widget);
            assert_eq!(size.width, Length::Fixed(18.0));
            assert_eq!(size.height, Length::Fixed(18.0));
        }
    }

    #[test]
    fn animated_check_clamps_progress_and_pulse() {
        let check = AnimatedCheck::new(16.0, 1.5, -0.2);
        assert_eq!(check.draw_progress, 1.0);
        assert_eq!(check.pop_pulse, 0.0);
        let size = <AnimatedCheck as Widget<(), Theme, iced::Renderer>>::size(&check);
        assert_eq!(size.width, Length::Fixed(16.0));
        assert_eq!(size.height, Length::Fixed(16.0));
    }
}
