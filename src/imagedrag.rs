//! Rekenregels voor het verslepen en schalen van een geplaatste afbeelding.
//! Alles in paginaruimte: punten, oorsprong linksboven, zonder zoom.

use eframe::egui::{Pos2, Rect, Vec2};

/// Kleinste zijde die een afbeelding mag krijgen, in punten.
const MIN_SIDE: f32 = 12.0;

/// De vier hoeken in de volgorde linksboven, rechtsboven, rechtsonder, linksonder.
pub fn corners(rect: Rect) -> [Pos2; 4] {
    [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()]
}

/// Verschuift `start` met `delta`, zonder dat de afbeelding de pagina verlaat.
pub fn moved(start: Rect, delta: Vec2, page: Vec2) -> Rect {
    let dx = delta.x.clamp(-start.min.x, (page.x - start.max.x).max(-start.min.x));
    let dy = delta.y.clamp(-start.min.y, (page.y - start.max.y).max(-start.min.y));
    start.translate(Vec2::new(dx, dy))
}

/// Schaalt `start` door aan hoek `corner` te trekken. De tegenoverliggende hoek
/// blijft staan, de verhouding blijft gelijk en de afbeelding blijft op de pagina.
pub fn resized(start: Rect, corner: usize, pointer: Pos2, page: Vec2) -> Rect {
    let points = corners(start);
    let anchor = points[(corner + 2) % 4];
    let dragged = points[corner % 4];
    let (sx, sy) = ((dragged.x - anchor.x).signum(), (dragged.y - anchor.y).signum());
    let (w, h) = (start.width().max(0.01), start.height().max(0.01));
    let wanted = ((pointer.x - anchor.x) * sx / w).max((pointer.y - anchor.y) * sy / h);
    // Ruimte tussen het ankerpunt en de paginarand in de richting waarin geschaald wordt.
    let room_x = if sx > 0.0 { page.x - anchor.x } else { anchor.x };
    let room_y = if sy > 0.0 { page.y - anchor.y } else { anchor.y };
    let largest = (room_x / w).min(room_y / h).max(0.01);
    let smallest = (MIN_SIDE / w.min(h)).min(largest);
    let scale = wanted.clamp(smallest, largest);
    Rect::from_two_pos(anchor, anchor + Vec2::new(sx * w * scale, sy * h * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: Vec2 = Vec2::new(400.0, 600.0);

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h))
    }

    #[test]
    fn moving_stays_on_the_page() {
        let start = rect(50.0, 60.0, 200.0, 100.0);
        assert_eq!(moved(start, Vec2::new(30.0, 40.0), PAGE), rect(80.0, 100.0, 200.0, 100.0));
        assert_eq!(moved(start, Vec2::new(-500.0, -500.0), PAGE), rect(0.0, 0.0, 200.0, 100.0));
        assert_eq!(moved(start, Vec2::new(900.0, 900.0), PAGE), rect(200.0, 500.0, 200.0, 100.0));
    }

    #[test]
    fn resizing_keeps_the_aspect_ratio_and_the_opposite_corner() {
        let start = rect(50.0, 60.0, 200.0, 100.0);
        // Rechtsonder naar buiten trekken: linksboven blijft staan.
        let r = resized(start, 2, Pos2::new(350.0, 200.0), PAGE);
        assert_eq!(r.min, start.min);
        assert!((r.width() / r.height() - 2.0).abs() < 1e-4);
        assert!((r.width() - 300.0).abs() < 1e-3);
        // Linksboven naar binnen trekken: rechtsonder blijft staan.
        let r = resized(start, 0, Pos2::new(150.0, 110.0), PAGE);
        assert_eq!(r.max, start.max);
        assert!((r.width() - 100.0).abs() < 1e-3 && (r.height() - 50.0).abs() < 1e-3);
    }

    #[test]
    fn resizing_is_limited_by_the_page_and_a_minimum_size() {
        let start = rect(50.0, 60.0, 200.0, 100.0);
        let r = resized(start, 2, Pos2::new(5000.0, 5000.0), PAGE);
        assert!(r.max.x <= 400.001 && r.max.y <= 600.001, "{r:?}");
        assert!((r.width() / r.height() - 2.0).abs() < 1e-4);
        // Voorbij het ankerpunt trekken klapt de afbeelding niet om en maakt hem niet onzichtbaar.
        let r = resized(start, 2, Pos2::new(0.0, 0.0), PAGE);
        assert_eq!(r.min, start.min);
        assert!((r.height() - 12.0).abs() < 1e-3 && (r.width() - 24.0).abs() < 1e-3, "{r:?}");
    }
}
