use crate::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum IpeTaskBackoffStrategy {
    Linear,
    LinearWithJitter,
    Exponential,
    ExponentialWithJitter,
}
impl IpeStringify for IpeTaskBackoffStrategy {
    fn ipe_show(&self) -> String {
        match self {
            IpeTaskBackoffStrategy::Linear => "Linear".to_string(),
            IpeTaskBackoffStrategy::LinearWithJitter => "LinearWithJitter".to_string(),
            IpeTaskBackoffStrategy::Exponential => "Exponential".to_string(),
            IpeTaskBackoffStrategy::ExponentialWithJitter => "ExponentialWithJitter".to_string(),
        }
    }
}
