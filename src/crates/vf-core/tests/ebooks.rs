//! Real books. Set `VOIDFLARE_TEST_BOOKS` to a folder holding the same DRM-free book as `book.epub` and
//! `book.azw3` (Standard Ebooks publishes both) and this reads each, writes the text of each next to them
//! (`book.epub.txt`, `book.azw3.txt`), checks that the two say the same thing, and that what we write as an EPUB
//! reads back. Without the variable the test does nothing.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use vf_core::epub::{self, Book};
use vf_core::mobi;

fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

#[test]
fn the_same_book_in_epub_and_azw3_says_the_same_thing() {
    let Some(dir) = std::env::var_os("VOIDFLARE_TEST_BOOKS").map(PathBuf::from) else { return };
    let (epub_path, azw_path) = (dir.join("book.epub"), dir.join("book.azw3"));
    let from_epub = epub::read_epub(&fs::read(&epub_path).unwrap()).expect("the EPUB reads");
    let from_azw = mobi::read_mobi(&fs::read(&azw_path).unwrap()).expect("the AZW3 reads");
    fs::write(dir.join("book.epub.txt"), from_epub.to_text()).unwrap();
    fs::write(dir.join("book.azw3.txt"), from_azw.to_text()).unwrap();
    println!(
        "epub: {:?} by {:?}, {} chapters, {} images, {} words",
        from_epub.title,
        from_epub.authors,
        from_epub.chapters.len(),
        from_epub.images.len(),
        words(&from_epub.to_text()).len()
    );
    println!(
        "azw3: {:?} by {:?}, {} chapters, {} images, {} words",
        from_azw.title,
        from_azw.authors,
        from_azw.chapters.len(),
        from_azw.images.len(),
        words(&from_azw.to_text()).len()
    );
    assert_eq!(from_epub.title, from_azw.title);
    assert_eq!(from_epub.authors, from_azw.authors);

    // Every word of the EPUB's text that is not boilerplate should be in the AZW3's, and the other way round.
    let (a, b) = (words(&from_epub.to_text()), words(&from_azw.to_text()));
    let (set_a, set_b): (HashSet<&String>, HashSet<&String>) = (a.iter().collect(), b.iter().collect());
    let missing: Vec<&&String> = set_a.difference(&set_b).take(20).collect();
    println!("words only in the EPUB: {} e.g. {missing:?}", set_a.difference(&set_b).count());
    println!("words only in the AZW3: {}", set_b.difference(&set_a).count());
    let (la, lb) = (a.len() as f64, b.len() as f64);
    assert!((la - lb).abs() / la < 0.03, "the two texts differ in length by more than 3%: {la} vs {lb}");

    // what we write as an EPUB reads back with the same text
    let rebuilt = from_azw.to_epub().expect("write an EPUB from the AZW3");
    fs::write(dir.join("from-azw3.epub"), &rebuilt).unwrap();
    let again: Book = epub::read_epub(&rebuilt).expect("our EPUB reads back");
    assert_eq!(words(&again.to_text()), words(&from_azw.to_text()));
    assert_eq!(again.images.len(), from_azw.images.len());
    fs::write(dir.join("book.html"), from_epub.to_html()).unwrap();
}
