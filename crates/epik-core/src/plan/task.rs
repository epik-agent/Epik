use anyhow::Result;

/// Work that can be run from any thread.
pub trait Task: Send + Sync {
    type Return;
    fn run(&self) -> Result<Self::Return>;
}

#[cfg(test)]
mod tests {
    use super::Task;
    use anyhow::Result;
    use std::fmt::{self, Display, Formatter};

    struct Division {
        x: f32,
        y: f32,
    }

    impl Division {
        fn new(x: f32, y: f32) -> Self {
            Self { x, y }
        }
    }

    #[derive(Debug)]
    struct DivideByZero;

    impl Display for DivideByZero {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str("division by zero")
        }
    }

    impl std::error::Error for DivideByZero {}

    impl Task for Division {
        type Return = f32;

        fn run(&self) -> Result<f32> {
            if self.y == 0.0 {
                Err(DivideByZero.into())
            } else {
                Ok(self.x / self.y)
            }
        }
    }

    #[test]
    fn ten_over_two_succeeds() {
        assert_eq!(Division::new(10.0, 2.0).run().unwrap(), 5.0);
    }

    #[test]
    fn ten_over_zero_fails() {
        let error = Division::new(10.0, 0.0).run().unwrap_err();
        assert!(error.is::<DivideByZero>(), "{error}");
    }
}
