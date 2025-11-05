use std::fmt;

/// A unique alias identifier for a file (e.g., "A", "AB", "XYZ")
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileAlias {
    /// The character string representation (stored as bytes for Copy)
    /// Max 3 bytes for "ZZZ", so we use a fixed array
    bytes: [u8; 3],
    /// Actual length used (1-3)
    len: u8,
}

impl FileAlias {
    /// Create a new FileAlias from a slice of chars
    pub fn new(chars: &[char]) -> Self {
        let len = chars.len().min(3);
        let mut bytes = [0u8; 3];

        for (i, &c) in chars.iter().take(len).enumerate() {
            // Store ASCII byte for each char (assumes A-Z)
            bytes[i] = c as u8;
        }

        Self {
            bytes,
            len: len as u8,
        }
    }

    /// Create a new FileAlias from a string
    pub fn from_str(s: impl AsRef<str>) -> Self {
        let s = s.as_ref();
        let len = s.len().min(3);
        let mut bytes = [0u8; 3];
        bytes[..len].copy_from_slice(&s.as_bytes()[..len]);

        Self {
            bytes,
            len: len as u8,
        }
    }

    /// Get the string value of this alias
    pub fn val(&self) -> String {
        String::from_utf8_lossy(&self.bytes[..self.len as usize]).into_owned()
    }

    /// Get the string value as a &str
    pub fn as_str(&self) -> &str {
        // SAFETY: We only store valid ASCII letters A-Z
        unsafe { std::str::from_utf8_unchecked(&self.bytes[..self.len as usize]) }
    }
}

impl fmt::Display for FileAlias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Use the string formatting to respect width, alignment, etc.
        fmt::Display::fmt(self.as_str(), f)
    }
}

impl PartialOrd for FileAlias {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for FileAlias {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare by length first (A < AA < AAA), then lexicographically
        self.len
            .cmp(&other.len)
            .then_with(|| self.bytes[..self.len as usize].cmp(&other.bytes[..other.len as usize]))
    }
}
