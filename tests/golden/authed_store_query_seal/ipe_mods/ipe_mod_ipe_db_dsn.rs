use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbDsnDriver {
    Postgres,
    Sqlite,
}
impl IpeStringify for IpeDbDsnDriver {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbDsnDriver::Postgres => "Postgres".to_string(),
            IpeDbDsnDriver::Sqlite => "Sqlite".to_string(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDbDsnTlsMode {
    Require,
    Prefer,
    Disable,
}
impl IpeStringify for IpeDbDsnTlsMode {
    fn ipe_show(&self) -> String {
        match self {
            IpeDbDsnTlsMode::Require => "Require".to_string(),
            IpeDbDsnTlsMode::Prefer => "Prefer".to_string(),
            IpeDbDsnTlsMode::Disable => "Disable".to_string(),
        }
    }
}
