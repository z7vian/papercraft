use crate::CoreError;
use mail_parser::MessageParser;

/// A saved web page (.mht/.mhtml): the main HTML plus the images/CSS it uses.
#[derive(uniffi::Record)]
pub struct WebArchive {
    pub html: String,
    /// Where the page came from; relative links in `html` resolve against it.
    pub base_url: Option<String>,
    pub resources: Vec<WebResource>,
}

#[derive(uniffi::Record)]
pub struct WebResource {
    /// Every URL this part answers to: its Content-Location and/or `cid:<Content-ID>`.
    pub urls: Vec<String>,
    pub mime: String,
    pub data: Vec<u8>,
}

#[uniffi::export]
pub fn parse_mhtml(bytes: Vec<u8>) -> Result<WebArchive, CoreError> {
    let bad = || CoreError::BadFile("Not a valid web archive (.mht)".into());
    let msg = MessageParser::default().parse(&bytes).ok_or_else(bad)?;
    let html = msg.body_html(0).ok_or_else(bad)?.into_owned();
    let base_url = msg.html_body().next().and_then(|p| p.content_location()).map(String::from);

    let resources = msg
        .parts()
        .filter(|p| !p.is_multipart() && !p.in_html_body())
        .filter_map(|p| {
            let mut urls: Vec<String> = p.content_location().map(String::from).into_iter().collect();
            if let Some(id) = p.content_id() {
                urls.push(format!("cid:{}", id.trim_matches(['<', '>'])));
            }
            let ct = p.content_type()?;
            let mime = format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("octet-stream"));
            (!urls.is_empty()).then(|| WebResource { urls, mime, data: p.decoded().into_owned() })
        })
        .collect();
    Ok(WebArchive { html, base_url, resources })
}

#[cfg(test)]
mod tests {
    use super::parse_mhtml;

    #[test]
    fn html_and_image() {
        let mht = b"MIME-Version: 1.0\r\n\
Content-Type: multipart/related; boundary=\"B\"; type=\"text/html\"\r\n\r\n\
--B\r\n\
Content-Type: text/html; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
Content-Location: https://example.com/page\r\n\r\n\
<p>caf=C3=A9</p><img src=3D\"a.png\">\r\n\
--B\r\n\
Content-Type: image/png\r\n\
Content-Transfer-Encoding: base64\r\n\
Content-Location: https://example.com/a.png\r\n\
Content-ID: <img1@x>\r\n\r\n\
aGk=\r\n\
--B--\r\n";
        let a = parse_mhtml(mht.to_vec()).unwrap();
        assert!(a.html.contains("<p>café</p>"), "{}", a.html);
        assert_eq!(a.base_url.as_deref(), Some("https://example.com/page"));
        assert_eq!(a.resources.len(), 1);
        assert_eq!(a.resources[0].urls, ["https://example.com/a.png", "cid:img1@x"]);
        assert_eq!(a.resources[0].mime, "image/png");
        assert_eq!(a.resources[0].data, b"hi");
    }

    #[test]
    fn junk_is_an_error() {
        assert!(parse_mhtml(b"hello".to_vec()).is_err());
    }
}
