use std::fmt;

#[derive(Debug, Clone)]
pub enum By {
    Name(String),
    Role(String),
    /// Natural-language description for vision fallback. Tried only if UIA misses.
    /// Example: "the blue Submit button at the bottom of the form"
    Description(String),
}

impl By {
    pub fn name(n: impl Into<String>) -> Self {
        By::Name(n.into())
    }

    pub fn role(r: impl Into<String>) -> Self {
        By::Role(r.into())
    }

    pub fn description(d: impl Into<String>) -> Self {
        By::Description(d.into())
    }
}

impl fmt::Display for By {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            By::Name(n) => write!(f, "name={}", n),
            By::Role(r) => write!(f, "role={}", r),
            By::Description(d) => write!(f, "description={}", d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn by_name_stores_name() {
        let loc = By::name("Save");
        assert!(matches!(loc, By::Name(n) if n == "Save"));
    }

    #[test]
    fn by_role_stores_role() {
        let loc = By::role("edit");
        assert!(matches!(loc, By::Role(r) if r == "edit"));
    }

    #[test]
    fn by_name_display() {
        assert_eq!(By::name("OK").to_string(), "name=OK");
    }

    #[test]
    fn by_role_display() {
        assert_eq!(By::role("button").to_string(), "role=button");
    }
}
