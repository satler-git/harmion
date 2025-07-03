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
}
