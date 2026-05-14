impl AsRef<std::path::Path> for Path {
    fn as_ref(&self) -> &std::path::Path {
        self.0.as_ref()
    }
}

impl AsRef<str> for Path {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}
