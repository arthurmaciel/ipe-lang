use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeNetPort {
    Port(i64),
}
impl IpeStringify for IpeNetPort {
    fn ipe_show(&self) -> String {
        match self {
            IpeNetPort::Port(p0) => {
                format!("Port {}", (&ipe_runtime::stringify::Wrap(p0)).dispatch())
            }
        }
    }
}
