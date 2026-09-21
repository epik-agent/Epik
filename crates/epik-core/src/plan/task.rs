use anyhow::Result;

/// Work that can be run from any thread: its future parks wherever it
/// waits, and a failure is its error.
pub trait Task: Send + Sync {
    fn run(&self) -> impl Future<Output = Result<()>> + Send;
}

#[cfg(test)]
mod tests {
    use super::Task;
    use anyhow::Result;
    use std::fmt::{self, Display, Formatter};

    /// Checks that it can be divided by.
    struct Divisor(f32);

    #[derive(Debug)]
    struct DivideByZero;

    impl Display for DivideByZero {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            f.write_str("division by zero")
        }
    }

    impl std::error::Error for DivideByZero {}

    impl Task for Divisor {
        async fn run(&self) -> Result<()> {
            if self.0 == 0.0 {
                Err(DivideByZero.into())
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn two_succeeds() {
        Divisor(2.0).run().await.unwrap();
    }

    #[tokio::test]
    async fn zero_fails() {
        let error = Divisor(0.0).run().await.unwrap_err();
        assert!(error.is::<DivideByZero>(), "{error}");
    }
}
