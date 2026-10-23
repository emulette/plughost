use super::*;

#[test]
fn any_rectangle_a_plugin_reports_has_dimensions() {
    let rect = |left, top, right, bottom| ViewRect {
        left,
        top,
        right,
        bottom,
    };
    assert_eq!(dimensions(&rect(10, 20, 410, 320)), (400, 300));
    assert_eq!(
        dimensions(&rect(i32::MIN, i32::MIN, i32::MAX, i32::MAX)),
        (u32::MAX, u32::MAX)
    );
    // Inverted or overflowing rectangles are empty, not a panic.
    assert_eq!(dimensions(&rect(i32::MAX, 0, i32::MIN, 0)), (0, 0));
}
