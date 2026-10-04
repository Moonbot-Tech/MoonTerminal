use super::*;

#[test]
fn the_form_carries_the_chat_the_caption_and_the_picture() {
    let png = [0x89, b'P', b'N', b'G', 0, 1, 2];
    let body = multipart("B", -100, "<b>1</b>", &png);
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.starts_with("--B\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n-100\r\n")
    );
    assert!(text.contains("name=\"caption\"\r\n\r\n<b>1</b>\r\n"));
    assert!(text.contains("name=\"parse_mode\"\r\n\r\nHTML\r\n"));
    assert!(text.contains("filename=\"deal.png\"\r\nContent-Type: image/png\r\n\r\n"));
    assert!(body.windows(png.len()).any(|w| w == png));
    assert!(text.ends_with("\r\n--B--\r\n"));
}

#[test]
fn an_empty_caption_sends_the_picture_alone() {
    let body = multipart("B", 1, "", &[1, 2, 3]);
    let text = String::from_utf8_lossy(&body);
    assert!(!text.contains("caption"));
    assert!(!text.contains("parse_mode"));
}

#[test]
fn the_boundary_never_occurs_in_the_picture() {
    let first = boundary(&[]);
    // A picture that happens to hold the first candidate gets another one.
    let png = first.as_bytes().to_vec();
    let picked = boundary(&png);
    assert!(!picked.is_empty());
    assert!(!contains(&png, picked.as_bytes()));
}
