use serde::{Deserialize, Serialize};

fn default_page() -> i64 {
    1
}

fn default_page_size() -> i64 {
    20
}

#[derive(Debug, Clone, Deserialize)]
pub struct PageQuery {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_page_size")]
    pub page_size: i64,
}

impl PageQuery {
    pub fn resolve(&self) -> (i64, i64, i64) {
        let page = self.page.max(1);
        let page_size = self.page_size.clamp(1, 100);
        let offset = (page - 1).saturating_mul(page_size);
        (page, page_size, offset)
    }
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, total: i64, page: i64, page_size: i64) -> Self {
        Self {
            items,
            total,
            page,
            page_size,
        }
    }
}
