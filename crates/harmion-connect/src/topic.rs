pub trait Topic {
    fn value(&self) -> &str;
}

impl<T: AsRef<str>> Topic for T {
    fn value(&self) -> &str {
        self.as_ref()
    }
}

// child, parent
pub struct TopicTree(pub Vec<Box<dyn Topic>>);

use std::fmt;

impl fmt::Debug for TopicTree {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(
            f,
            "{}",
            self.0
                .iter()
                .rev()
                .map(|i| i.value())
                .collect::<Vec<_>>()
                .join(" / ")
        )?; // TODO: Itertools
        Ok(())
    }
}

// topic!("/to/file" / harmion::channel::builtin::Write)
// Write -> harmion/ ...
// parent: None
#[macro_export]
macro_rules! topic {
    ( $single:expr ) => {{
        let mut v = Vec::new();
        v.push(Box::new($single) as Box<dyn Topic>);
        TopicTree(v)
    }};
    // 再帰ケース
    ( $head:expr , $($rest:expr),+ ) => {{
        let mut tree = $crate::topic!($($rest),+).0;
        tree.push(Box::new($head) as Box<dyn Topic>);
        TopicTree(tree)
    }};
}

#[cfg(test)]
mod tests {
    use super::{Topic, TopicTree};

    #[test]
    fn topic_macro() -> Result<(), Box<dyn std::error::Error>> {
        let mut tt = crate::topic!("harmion", "write").0;

        assert_eq!(tt.len(), 2);
        assert_eq!(tt.pop().unwrap().value(), "harmion");
        Ok(())
    }

    #[test]
    fn debug_topic_tree() -> Result<(), Box<dyn std::error::Error>> {
        let topic = crate::topic!("harmion", "write");
        assert_eq!(format!("{:?}", topic), "harmion / write\n");

        let topic = crate::topic!("harmion", "write", "/to/file");
        assert_eq!(format!("{:?}", topic), "harmion / write / /to/file\n");

        Ok(())
    }
}
