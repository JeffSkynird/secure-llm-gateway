use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Default, Debug, Clone, Copy)]
pub struct RedactionStats {
    pub matches: usize,
}
impl std::ops::AddAssign for RedactionStats {
    fn add_assign(&mut self, rhs: Self) {
        self.matches += rhs.matches;
    }
}

pub fn redact_text(input: &str) -> (String, RedactionStats) {
    let mut out = input.to_string();
    let mut stats = RedactionStats::default();

    // Emails
    static EMAIL: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)\b([A-Z0-9._%+-]+)@([A-Z0-9.-]+\.[A-Z]{2,})\b").unwrap());
    out = EMAIL
        .replace_all(&out, |caps: &regex::Captures| {
            stats.matches += 1;
            let user = &caps[1];
            let domain = &caps[2];
            let masked_user = mask_mid(user, 1);
            format!("{}@{}", masked_user, domain)
        })
        .to_string();

    // Possible credit card numbers: sequences of 12-19 digits optionally separated by spaces/dashes
    static CC: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(?:\d[ -]*?){12,19}\b").unwrap());
    out = CC
        .replace_all(&out, |caps: &regex::Captures| {
            let raw = caps.get(0).unwrap().as_str();
            let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
            if luhn_check(&digits) {
                stats.matches += 1;
                let last4 = &digits[digits.len().saturating_sub(4)..];
                format!("CC_MASKED_LAST4_{}", last4)
            } else {
                raw.to_string()
            }
        })
        .to_string();

    // Phone numbers: mask groups of 7-15 digits
    static PHONE: Lazy<Regex> = Lazy::new(|| {
        // Match phone numbers that are not preceded by another digit (look-behind unsupported)
        Regex::new(r"(?m)(^|[^\d])(\+?\d[\d \-]{6,}\d)").unwrap()
    });
    out = PHONE
        .replace_all(&out, |caps: &regex::Captures| {
            stats.matches += 1;
            let prefix = caps.get(1).unwrap().as_str();
            let raw = caps.get(2).unwrap().as_str();
            format!("{}{}", prefix, mask_digits(raw))
        })
        .to_string();

    (out, stats)
}

fn mask_mid(s: &str, keep: usize) -> String {
    if s.len() <= keep {
        return "*".repeat(s.len());
    }
    let mut out = String::with_capacity(s.len());
    for (i, ch) in s.chars().enumerate() {
        if i < keep || i >= s.len().saturating_sub(keep) {
            out.push(ch);
        } else {
            out.push('*');
        }
    }
    out
}

fn mask_digits(s: &str) -> String {
    let mut count = 0;
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            count += 1;
            if count > 2 {
                out.push('x');
            } else {
                out.push(ch);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

// Luhn algorithm for validating credit card numbers
pub fn luhn_check(digits: &str) -> bool {
    if digits.len() < 12 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0;
    let mut alternate = false;
    for ch in digits.chars().rev() {
        if let Some(mut n) = ch.to_digit(10) {
            if alternate {
                n *= 2;
                if n > 9 {
                    n -= 9;
                }
            }
            sum += n;
            alternate = !alternate;
        } else {
            return false;
        }
    }
    sum % 10 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_luhn() {
        assert!(luhn_check("4242424242424242"));
        assert!(!luhn_check("1234567890123456"));
    }
}
