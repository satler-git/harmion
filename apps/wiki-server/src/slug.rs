//! Deterministic, filesystem-safe file stem helper.

/// Lowercase alphanumerics (Unicode preserved), collapse other runs to `-`.
/// The result is never empty.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for character in title.chars() {
        if character.is_alphanumeric() {
            for lower in character.to_lowercase() {
                out.push(lower);
            }
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "untitled".to_owned()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_stable_and_safe() {
        assert_eq!(slugify("家計の記録"), "家計の記録");
        assert_eq!(slugify("Hello, World / 2026"), "hello-world-2026");
        assert_eq!(slugify("  __  "), "untitled");
        assert_eq!(slugify("a//b"), "a-b");
    }
}
