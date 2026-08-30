use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeDurationDuration {
    Duration(i64),
}
impl IpeStringify for IpeDurationDuration {
    fn ipe_show(&self) -> String {
        match self {
            IpeDurationDuration::Duration(p0) => format!(
                "Duration {}",
                (&ipe_runtime::stringify::Wrap(p0)).dispatch()
            ),
        }
    }
}
