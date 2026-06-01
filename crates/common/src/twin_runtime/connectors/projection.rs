#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionError {
    InvalidPayload(&'static str),
}

pub trait Projector<In, Out> {
    fn project(&self, input: In) -> Result<Out, ProjectionError>;
}
