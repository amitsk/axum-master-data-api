use crate::domain::{normalize_code, FieldError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListFilter {
    pub code: Option<String>,
    pub name: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl ListFilter {
    pub const DEFAULT_LIMIT: i64 = 50;
    pub const MAX_LIMIT: i64 = 500;

    pub fn new(
        code: Option<String>,
        name: Option<String>,
        limit: Option<i64>,
        offset: Option<i64>,
    ) -> Result<Self, Vec<FieldError>> {
        let mut errors = Vec::new();
        let limit = limit.unwrap_or(Self::DEFAULT_LIMIT);
        if !(1..=Self::MAX_LIMIT).contains(&limit) {
            errors.push(FieldError::new(
                "limit",
                format!("must be between 1 and {}", Self::MAX_LIMIT),
            ));
        }
        let offset = offset.unwrap_or(0);
        if offset < 0 {
            errors.push(FieldError::new("offset", "must be >= 0"));
        }
        if !errors.is_empty() {
            return Err(errors);
        }
        let code = code.map(|c| normalize_code(&c)).filter(|c| !c.is_empty());
        let name = name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty());
        Ok(Self {
            code,
            name,
            limit,
            offset,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub limit: i64,
    pub offset: i64,
    pub total: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_when_absent() {
        let f = ListFilter::new(None, None, None, None).unwrap();
        assert_eq!(f.limit, 50);
        assert_eq!(f.offset, 0);
        assert_eq!(f.code, None);
    }

    #[test]
    fn code_filter_is_normalized_and_blank_name_dropped() {
        let f = ListFilter::new(Some(" us ".into()), Some("   ".into()), Some(10), Some(5)).unwrap();
        assert_eq!(f.code.as_deref(), Some("US"));
        assert_eq!(f.name, None);
        assert_eq!((f.limit, f.offset), (10, 5));
    }

    #[test]
    fn limit_bounds_are_enforced_not_clamped() {
        for bad in [0, -1, 501, 10_000] {
            let errs = ListFilter::new(None, None, Some(bad), None).unwrap_err();
            assert_eq!(errs[0].field, "limit", "limit={bad}");
        }
        assert!(ListFilter::new(None, None, Some(500), None).is_ok());
    }

    #[test]
    fn negative_offset_rejected() {
        let errs = ListFilter::new(None, None, None, Some(-1)).unwrap_err();
        assert_eq!(errs[0].field, "offset");
    }
}
