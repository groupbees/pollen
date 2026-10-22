//! `pollen schema`: print the JSON Schema for `pollen.yaml`.

use anyhow::Result;

use crate::schema;

/// Run the subcommand.
///
/// # Errors
///
/// When the schema document cannot be serialised.
pub fn run() -> Result<()> {
    print!("{}", schema::rendered()?);
    Ok(())
}
