use anyhow::Result;

use crate::schema;

/// # Errors
///
/// When the schema document cannot be serialised.
pub fn run() -> Result<()> {
    print!("{}", schema::rendered()?);
    Ok(())
}
