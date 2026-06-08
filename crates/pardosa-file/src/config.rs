#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PageClass {
    Page0 = 0,
    Page1 = 1,
    Page2 = 2,
    Page3 = 3,
}
impl PageClass {
    #[must_use]
    pub const fn max_elements(self) -> usize {
        match self {
            Self::Page0 => 256,
            Self::Page1 => 4_096,
            Self::Page2 => 65_536,
            Self::Page3 => 1_048_576,
        }
    }
    #[must_use]
    pub const fn from_byte(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Page0),
            1 => Some(Self::Page1),
            2 => Some(Self::Page2),
            3 => Some(Self::Page3),
            _ => None,
        }
    }
}
