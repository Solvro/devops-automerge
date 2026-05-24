use std::{
    fs::{self, File},
    io::Write,
};

use cloneable_errors::{ErrorContext, ResContext, bail};
use reqwest::StatusCode;

fn main() -> Result<(), ErrorContext> {
    let schema_path = "queries/schema.graphql";
    if !fs::exists(schema_path).is_ok_and(|x| x) {
        println!("cargo::warning=\"GitHub GraphQL schema missing - downloading\"");
        let mut file = File::options()
            .write(true)
            .create_new(true)
            .open(schema_path)
            .context("Failed to create new file 'github.schema' in the build dir and open it for writing")?;

        let mut resp =
            reqwest::blocking::get("https://docs.github.com/public/fpt/schema.docs.graphql")
                .context("Failed to fetch GitHub's public GraphQL schema")?;
        let status = resp.status();

        if status != StatusCode::OK {
            bail!("Fetching GitHub's public GraphQL schema failed: Got status code {status}",);
        }

        resp.copy_to(&mut file)
            .context("Failed to write GitHub's public GraphQL schema to github.schema")?;
        file.flush()
            .context("Failed to flush github.schema after writing")?;
        println!("cargo::warning=\"GitHub GraphQL download finished\"");
    }
    Ok(())
}
