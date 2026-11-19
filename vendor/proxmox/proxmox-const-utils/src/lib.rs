/// Note: this only compares *bytes* and is not strictly speaking equivalent to str::cmp!
pub const fn byte_string_cmp(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;

    // const-version of `min(a.len(), b.len())` while simultaneously remembering
    // `cmp(a.len(), b.len())`.
    let (end, len_result) = if a.len() < b.len() {
        (a.len(), Less)
    } else if a.len() > b.len() {
        (b.len(), Greater)
    } else {
        (a.len(), Equal)
    };

    let mut i = 0;
    while i != end {
        if a[i] < b[i] {
            return Less;
        } else if a[i] > b[i] {
            return Greater;
        }
        i += 1;
    }
    len_result
}

/// As with [`byte_string_cmp`] this only compares bytes.
pub const fn byte_string_eq(a: &[u8], b: &[u8]) -> bool {
    matches!(byte_string_cmp(a, b), std::cmp::Ordering::Equal)
}

#[cfg(test)]
mod test {
    use crate::{byte_string_cmp, byte_string_eq};
    use std::cmp::Ordering::{self, *};

    fn str_cmp(a: &str, b: &str, res: Ordering) {
        assert_eq!(byte_string_cmp(a.as_bytes(), b.as_bytes()), res);
    }

    fn str_eq(a: &str, b: &str, res: bool) {
        assert_eq!(byte_string_eq(a.as_bytes(), b.as_bytes()), res);
    }

    #[test]
    fn test_cmp() {
        str_cmp("foo", "bar", Greater);
        str_cmp("bar", "foo", Less);
        str_cmp("foo", "fooo", Less);
        str_cmp("fooo", "foo", Greater);
        str_cmp("foo", "foo", Equal);
        str_cmp("", "", Equal);
        str_cmp("", "foo", Less);
        str_cmp("foo", "", Greater);
    }

    #[test]
    fn test_eq() {
        str_eq("", "", true);
        str_eq("foo", "foo", true);
        str_eq("foo", "", false);
        str_eq("", "foo", false);
        str_eq("bar", "foo", false);
        str_eq("foo", "bar", false);
    }
}
