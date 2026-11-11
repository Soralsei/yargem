#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConsoleMode {
    Dmg,
    DmgCompat,
    Cgb,
}

impl From<(bool, bool)> for ConsoleMode {
    fn from((cgb_flag, cgb_override): (bool, bool)) -> Self {
        match (cgb_flag, cgb_override) {
            (true, _) => Self::Cgb,
            (false, true) => Self::DmgCompat,
            (false, false) => Self::Dmg,
        }
    }
}
