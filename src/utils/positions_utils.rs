use regex::Regex;

pub fn clean_address(address: &str) -> String {
    // Remove emojis and other special characters
    let re = Regex::new(r"[^\w\s]").unwrap();
    let cleaned = re.replace_all(address, "");

    // Remove any leading/trailing whitespace and newlines
    cleaned.trim().lines().next().unwrap_or("").to_string()
}
