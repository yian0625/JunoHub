pub fn get() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .connect_timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("OAuth HTTP client")
        })
        .clone()
}

// Test-local endpoint substitution; not available in deployed builds.
#[cfg(test)]
tokio::task_local! {pub static TEST_ENDPOINT:String;}
pub fn endpoint(url: &str) -> String {
    #[cfg(test)]
    if let Ok(base) = TEST_ENDPOINT.try_with(Clone::clone) {
        return format!(
            "{}{}",
            base,
            url::Url::parse(url).expect("official OAuth URL").path()
        );
    }
    url.to_string()
}
