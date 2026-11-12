//! Minimal snake_case → PascalCase + singularize pass for Rails association
//! arguments.
//!
//! This is intentionally much smaller than ActiveSupport's inflector.
//! Coverage:
//!
//! - `:user` → `User`
//! - `:user_role` → `UserRole`
//! - `:comments` (plural = true) → `Comment`
//! - `:companies` (plural = true) → `Company`
//! - `:user_roles` (plural = true) → `UserRole`
//!
//! Unknown irregular plurals (`people`, `children`, `octopi`) fall back to
//! naïve `-s`/`-es`/`-ies` rules and will produce wrong class names for
//! those. A richer inflector lands with the sf-ruby full extractor; this
//! first slice covers the common 95%.

/// Inflect an association symbol into its implied class name.
///
/// `name` is the symbol as written (`"user"`, `"user_roles"`).
/// `plural` is true for `has_many` and `has_and_belongs_to_many`; the name
/// gets singularized before the Pascal pass.
pub fn associate_to_class(name: &str, plural: bool) -> String {
    let singular = if plural {
        singularize(name)
    } else {
        name.to_string()
    };
    pascalize(&singular)
}

fn singularize(name: &str) -> String {
    if let Some(stem) = name.strip_suffix("ies") {
        return format!("{stem}y");
    }
    if let Some(stem) = name.strip_suffix("ses") {
        return format!("{stem}s");
    }
    if let Some(stem) = name.strip_suffix('s') {
        return stem.to_string();
    }
    name.to_string()
}

fn pascalize(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for part in name.split('_') {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            for c in first.to_uppercase() {
                out.push(c);
            }
            out.push_str(chars.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_symbol() {
        assert_eq!(associate_to_class("user", false), "User");
    }

    #[test]
    fn snake_case() {
        assert_eq!(associate_to_class("user_role", false), "UserRole");
    }

    #[test]
    fn plural_trailing_s() {
        assert_eq!(associate_to_class("comments", true), "Comment");
    }

    #[test]
    fn plural_ies_to_y() {
        assert_eq!(associate_to_class("companies", true), "Company");
    }

    #[test]
    fn plural_snake_case() {
        assert_eq!(associate_to_class("user_roles", true), "UserRole");
    }

    #[test]
    fn plural_flag_is_safe_on_nonplural_names() {
        // `user` matches none of the suffix patterns (not `s`, `es`, or `ies`),
        // so passing `plural=true` leaves it alone — we don't over-strip.
        assert_eq!(associate_to_class("user", true), "User");
    }
}
