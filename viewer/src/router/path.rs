use std::{
    borrow::Borrow,
    collections::BTreeMap,
    convert::Infallible,
    str::{FromStr, Split},
};

use url::form_urlencoded::{self, Parse};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Path {
    path: String,
    query: Option<String>,
    // Hash
    fragment: Option<String>,
}

impl Path {
    pub fn parse(path: &str) -> Self {
        path.parse().unwrap()
    }

    pub fn with_params<Item: Borrow<(K, V)>, K: AsRef<str>, V: AsRef<str>>(
        path: &str,
        params: impl IntoIterator<Item = Item>,
    ) -> Self {
        let mut path = path.to_string();
        let query = form_urlencoded::Serializer::new(String::new())
            .extend_pairs(params)
            .finish();
        if !query.is_empty() {
            path.push('?');
            path.push_str(&query);
        }
        path.parse().unwrap()
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn query(&self) -> Option<&str> {
        self.query.as_deref()
    }

    pub fn fragment(&self) -> Option<&str> {
        self.fragment.as_deref()
    }

    pub fn path_segments(&self) -> Option<Split<'_, char>> {
        self.path()
            .strip_prefix('/')
            .map(|remainder| remainder.split('/'))
    }

    pub fn query_iter(&self) -> Parse<'_> {
        form_urlencoded::parse(self.query.as_deref().unwrap_or("").as_bytes())
    }

    pub fn query_pairs(&self) -> BTreeMap<String, String> {
        self.query_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}

impl FromStr for Path {
    type Err = Infallible;

    fn from_str(path: &str) -> Result<Self, Self::Err> {
        let mut path = path.to_string();
        let mut query = None;
        let mut fragment = None;

        if let Some(pos) = path.find('#') {
            fragment = Some(path.split_off(pos)[1..].to_string());
        }

        if let Some(pos) = path.find('?') {
            query = Some(path.split_off(pos)[1..].to_string());
        }

        Ok(Path {
            path,
            query,
            fragment,
        })
    }
}

impl From<String> for Path {
    fn from(path: String) -> Self {
        Self::parse(&path)
    }
}

impl From<&String> for Path {
    fn from(path: &String) -> Self {
        Self::parse(path)
    }
}

impl From<&str> for Path {
    fn from(path: &str) -> Self {
        Self::parse(path)
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)?;
        if let Some(query) = &self.query {
            write!(f, "?{}", query)?;
        }
        if let Some(fragment) = &self.fragment {
            write!(f, "#{}", fragment)?;
        }
        Ok(())
    }
}
