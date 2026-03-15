use url::Url;

use crate::{
    ExportsPublicError, Result, add_data_suffix, add_resolution_suffix, has_data_suffix,
    has_resolution_suffix, strip_data_suffix, strip_resolution_suffix,
};

#[derive(Clone, Copy)]
enum NormalizeKind {
    None,
    TrailingSlash,
    Data,
    Resolution,
}

pub struct NormalizedUrl {
    pub url: Url,
    pub was_normalized: bool,
    base: Url,
    kind: NormalizeKind,
}

impl NormalizedUrl {
    pub fn denormalize(&self, next: Option<&str>) -> Result<Url> {
        let mut next_url = match next {
            None => self.base.clone(),
            Some(value) => {
                if let Ok(url) = Url::parse(value) {
                    url
                } else {
                    self.base
                        .join(value)
                        .map_err(|error| ExportsPublicError::DenormalizeUrl {
                            base: self.base.to_string(),
                            next: value.to_string(),
                            message: error.to_string(),
                        })?
                }
            }
        };

        match self.kind {
            NormalizeKind::Resolution => {
                let path = add_resolution_suffix(next_url.path());
                next_url.set_path(&path);
            }
            NormalizeKind::Data => {
                let path = add_data_suffix(next_url.path());
                next_url.set_path(&path);
            }
            NormalizeKind::TrailingSlash => {
                if !next_url.path().ends_with('/') {
                    let path = format!("{}/", next_url.path());
                    next_url.set_path(&path);
                }
            }
            NormalizeKind::None => {}
        }

        Ok(next_url)
    }
}

pub fn normalize_url(input: &str) -> Result<NormalizedUrl> {
    let mut url = Url::parse(input).map_err(|error| ExportsPublicError::NormalizeUrl {
        input: input.to_string(),
        message: error.to_string(),
    })?;

    let kind = if has_resolution_suffix(url.path()) {
        url.set_path(&strip_resolution_suffix(url.path()));
        NormalizeKind::Resolution
    } else if has_data_suffix(url.path()) {
        url.set_path(&strip_data_suffix(url.path()));
        NormalizeKind::Data
    } else if url.path() != "/" && url.path().ends_with('/') {
        let trimmed = url.path().trim_end_matches('/').to_string();
        url.set_path(&trimmed);
        NormalizeKind::TrailingSlash
    } else {
        NormalizeKind::None
    };

    let base = url.clone();

    Ok(NormalizedUrl {
        url,
        was_normalized: !matches!(kind, NormalizeKind::None),
        base,
        kind,
    })
}
