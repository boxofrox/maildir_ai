use std::str::FromStr;
use std::time::SystemTime;

use utf8path::Path;
use yammer::{FieldWriteAccumulator, GenerateRequest, Request, RequestOptions};

///////////////////////////////////////////// constants ////////////////////////////////////////////

const ARCHIVE: &str = "Archive";
const DRAFTS: &str = "Drafts";
const INBOX: &str = "INBOX";
const SENT: &str = "Sent";
const TRASH: &str = "Trash";

const CUR: &str = "cur";
const NEW: &str = "new";
const TMP: &str = "tmp";

/////////////////////////////////////////////// init ///////////////////////////////////////////////

/// Initialize a new maildir-ai database.
pub fn init(knowledge_base: &utf8path::Path<'_>, real_name: &str) -> Result<(), std::io::Error> {
    for level1 in &[ARCHIVE, DRAFTS, INBOX, SENT, TRASH] {
        std::fs::create_dir_all(knowledge_base.join(*level1).join(CUR))?;
        std::fs::create_dir_all(knowledge_base.join(*level1).join(NEW))?;
        std::fs::create_dir_all(knowledge_base.join(*level1).join(TMP))?;
    }
    let knowledge_base = Path::cwd()
        .unwrap_or(Path::from("."))
        .join(knowledge_base.clone());
    let muttrc = format!(
        r#"
set realname="{real_name}"
set envelope_from="yes"
set sendmail="/bin/true"
set my_status_format="-%r-Mutt: %f [Msgs:%?M?%M/?%m%?n? New:%n?%?o? Old:%o?%?d? Del:%d?%?F? Flag:%F?%?t? Tag:%t?%?p? Post:%p?%?b? Inc:%b?%?l? %l?]---(%s/%S)-%>-(%P)---"
set reverse_name=yes
set reverse_realname=no
set use_from=yes

#################################### Folders ###################################

set folder="{knowledge_base}"
set mbox_type=Maildir
set spoolfile="+INBOX"

set copy=yes
set move=no

set record="+Sent"
set postponed="+Drafts"
save-hook . "+Archive"

folder-hook "+.*" 'macro index d "<save-message>+Trash<enter><enter>"'
folder-hook "+Trash" 'macro index d <delete-message>'
macro index,pager a '<save-message>+Archive<enter><enter>'
set mask="!^\\.[^.]"

macro index,pager a '<save-message>+Archive<enter><enter>'

mailboxes `echo -n "+ "; find {knowledge_base} -maxdepth 1 -type d -name ".*" -printf "+'%f' "`

################################### Browsing ###################################

# Show mailboxes with unread, new mail.
macro index,pager y <change-folder>?<toggle-mailboxes>
# Stop at the end of messages
set pager_stop=yes
# show N index lines above the message when viewing it
set pager_index_lines=10
# sort messages in a nice way
set sort="threads"
set sort_aux="reverse-last-date-received"

################################### Composing ##################################

set edit_headers="yes"

###################################### Misc #####################################

auto_view text/x-vcard text/html text/enriched
set mark_old=no

# vim: filetype=muttrc
"#
    );
    std::fs::write(knowledge_base.join(".muttrc"), muttrc)?;
    Ok(())
}

////////////////////////////////////////// MaintainOptions /////////////////////////////////////////

/// The options for maintaining a knowledge base.
#[derive(Clone, Debug, Default, Eq, PartialEq, arrrg_derive::CommandLine)]
pub struct MaintainOptions {
    #[arrrg(nested)]
    yammer: RequestOptions,
}

///////////////////////////////////////////// maintain /////////////////////////////////////////////

/// Maintain a knowledge base.  This will try hard to not fail.
pub async fn maintain(options: &MaintainOptions, knowledge_base: &utf8path::Path<'_>) {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    tx.send(tokio::task::spawn(async move {})).await.unwrap();
    let _reap = tokio::task::spawn(async move {
        while let Some(handle) = rx.recv().await {
            let _ = handle.await;
        }
    });
    loop {
        if let Err(e) = maintain_one(options, knowledge_base).await {
            eprintln!("error: {}", e);
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

async fn maintain_one(
    options: &MaintainOptions,
    knowledge_base: &utf8path::Path<'_>,
) -> Result<(), std::io::Error> {
    for dirent in std::fs::read_dir(knowledge_base.join(SENT).join(CUR))? {
        let dirent = dirent?;
        let path = Path::try_from(dirent.path())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        if path.into_std().is_file() {
            let email = std::fs::read_to_string(&path)?;
            let Some(to) = extract_to(&email) else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("missing To header in email: {}", path),
                ));
            };
            let to = to
                .split(",")
                .map(|x| x.trim().to_string())
                .collect::<Vec<_>>();
            for to in to.into_iter() {
                let options = options.clone();
                let knowledge_base = knowledge_base.clone().into_owned();
                let path = path.clone();
                let email = email.clone();
                tokio::task::spawn(async move {
                    let email = match process_one(&options, &path, &to, &email).await {
                        Ok(email) => email,
                        Err(e) => match format_reply(&to, email.clone()) {
                            Ok(mut email) => {
                                email.push_str("\n\n");
                                email += &format!("error processing: {}", e);
                                email
                            }
                            Err(e) => {
                                format!("error processing: {}\n", e)
                            }
                        },
                    };
                    let now = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .expect("time should go forwards only");
                    let save_as = format!(
                        "{:0.5}_1.{}:2,",
                        now.as_secs_f64(),
                        std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string())
                    );
                    eprintln!("saving.... to {}", save_as);
                    let _ =
                        std::fs::write(knowledge_base.join(INBOX).join(CUR).join(&save_as), email);
                });
            }
            std::fs::rename(
                &path,
                knowledge_base.join(INBOX).join(CUR).join(path.basename()),
            )?;
        }
    }
    Ok(())
}

async fn process_one(
    options: &MaintainOptions,
    path: &Path<'_>,
    to: &str,
    email: &String,
) -> Result<String, std::io::Error> {
    // SAFETY(rescrv):  It will always return at least one string.
    let to = to.split("@").next().unwrap().to_string();
    eprintln!("processing: {} to {}", path, to);
    let generate = GenerateRequest {
        model: to.clone(),
        prompt: email.clone(),
        suffix: "".to_string(),
        system: None,
        stream: None,
        template: None,
        raw: None,
        format: None,
        images: None,
        keep_alive: None,
    };
    let mut buf = vec![];
    Request::generate(options.yammer.clone(), generate)?
        .accumulate(&mut FieldWriteAccumulator::new(&mut buf, "response"))
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{:?}", e)))?;
    let buf = String::from_utf8(buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{:?}", e)))?;
    let mut email = format_reply(&to, email)?;
    email.push_str("\n\n");
    fn wrap_line(line: &str) -> String {
        let mut offset = 0usize;
        let mut wrapped = String::new();
        let indent = if line.trim_start().starts_with("* ") {
            line.chars().count() - line.trim_start().chars().count() - 2
        } else {
            line.chars().count() - line.trim_start().chars().count()
        };
        for word in line.split_whitespace() {
            if wrapped.len() + word.len() - offset > 72 {
                wrapped.push('\n');
                offset = wrapped.len();
                wrapped.push_str(&" ".repeat(indent));
            }
            wrapped += word;
            wrapped.push(' ');
        }
        wrapped.push('\n');
        wrapped
    }
    fn wrap_answer(answer: &str) -> String {
        let mut wrapped = String::new();
        for line in answer.lines() {
            wrapped += &wrap_line(line);
        }
        wrapped
    }
    email += &wrap_answer(&buf);
    Ok(email)
}

////////////////////////////////////////////// Header //////////////////////////////////////////////

/// A Header likely to be produced by Mutt.
#[derive(Clone, Debug)]
pub enum Header {
    From(String),
    To(String),
    Cc(String),
    Subject(String),
    Date(String),
    MessageID(String),
    MimeVersion,
    ContentType,
    References(String),
    ContentDisposition,
    InReplyTo(String),
}

impl Header {
    fn from_block(header_block: impl AsRef<str>) -> Result<Vec<Self>, std::io::Error> {
        let mut headers = vec![];
        let mut current_line = "".to_string();
        for line in header_block.as_ref().lines() {
            if line.starts_with(' ') || line.starts_with('\t') {
                current_line.push_str(line);
            } else {
                if !current_line.is_empty() {
                    let Ok(header) = current_line.parse::<Header>() else {
                        continue;
                    };
                    headers.push(header);
                }
                current_line = line.to_string();
            }
        }
        if !current_line.is_empty() {
            if let Ok(header) = current_line.parse::<Header>() {
                headers.push(header);
            }
        }
        Ok(headers)
    }

    fn of_same_type(a: &Self, b: &Self) -> bool {
        matches!(
            (a, b),
            (Header::From(_), Header::From(_))
                | (Header::To(_), Header::To(_))
                | (Header::Cc(_), Header::Cc(_))
                | (Header::Subject(_), Header::Subject(_))
                | (Header::Date(_), Header::Date(_))
                | (Header::MessageID(_), Header::MessageID(_))
                | (Header::MimeVersion, Header::MimeVersion)
                | (Header::ContentType, Header::ContentType)
                | (Header::References(_), Header::References(_))
                | (Header::ContentDisposition, Header::ContentDisposition)
                | (Header::InReplyTo(_), Header::InReplyTo(_))
        )
    }
}

impl FromStr for Header {
    type Err = std::io::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(from) = s.strip_prefix("From: ") {
            Ok(Header::From(from.to_string()))
        } else if let Some(to) = s.strip_prefix("To: ") {
            Ok(Header::To(to.to_string()))
        } else if let Some(cc) = s.strip_prefix("Cc: ") {
            Ok(Header::Cc(cc.to_string()))
        } else if let Some(subject) = s.strip_prefix("Subject: ") {
            Ok(Header::Subject(subject.to_string()))
        } else if let Some(date) = s.strip_prefix("Date: ") {
            Ok(Header::Date(date.to_string()))
        } else if let Some(msg_id) = s.strip_prefix("Message-ID: ") {
            Ok(Header::MessageID(msg_id.to_string()))
        } else if s == "MIME-Version: 1.0" {
            Ok(Header::MimeVersion)
        } else if s == "Content-Type: text/plain; charset=us-ascii" {
            Ok(Header::ContentType)
        } else if let Some(refs) = s.strip_prefix("References: ") {
            Ok(Header::References(refs.to_string()))
        } else if s == "Content-Disposition: inline" {
            Ok(Header::ContentDisposition)
        } else if let Some(in_reply_to) = s.strip_prefix("In-Reply-To: ") {
            Ok(Header::InReplyTo(in_reply_to.to_string()))
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("invalid header: {}", s),
            ))
        }
    }
}

//////////////////////////////////////////// extract_to ////////////////////////////////////////////

/// Extract the To header from a message.
pub fn extract_to(message: impl AsRef<str>) -> Option<String> {
    let message = message.as_ref();
    let (header_block, _) = message.split_once("\n\n").unwrap_or(("", ""));
    let headers = Header::from_block(header_block).ok()?;
    if let Some(Header::To(to)) = headers
        .iter()
        .find(|header| matches!(header, Header::To(_)))
    {
        Some(to.clone())
    } else {
        None
    }
}

/////////////////////////////////////////// format_reply ///////////////////////////////////////////

/// Format a reply to an email as if the reply comes from "From".
pub fn format_reply(from: &str, message: impl AsRef<str>) -> Result<String, std::io::Error> {
    let message = message.as_ref();
    let (header_block, body) = message.split_once("\n\n").unwrap_or(("", ""));
    let mut headers = Header::from_block(header_block)?;
    let orig_headers = headers.clone();
    // rewrite the date header
    let date = chrono::Utc::now().to_rfc2822();
    let mut orig_date = None;
    for header in headers.iter_mut() {
        if let Header::Date(x) = header {
            if orig_date.is_none() {
                orig_date = Some(x.clone());
            }
            *x = date.clone();
        }
    }
    let Some(orig_date) = orig_date else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing Date header",
        ));
    };
    // turn every to into a cc
    for header in headers.iter_mut() {
        if let Header::To(x) = header {
            *header = Header::Cc(x.clone());
        }
    }
    // coalesce the cc headers
    let mut cc = None;
    headers.retain(|header| {
        if let Header::Cc(x) = header {
            if let Some(s) = &cc {
                cc = Some(format!("{}, {}", s, x));
            } else {
                cc = Some(x.clone());
            }
            false
        } else {
            true
        }
    });
    if let Some(cc) = cc {
        let mut pieces = cc.split(", ").collect::<Vec<_>>();
        pieces.retain(|x| *x != from);
        if !pieces.is_empty() {
            headers.push(Header::Cc(wrap_header(pieces.join(","))));
        }
    }
    // Set the To header to the original sender
    let Some(Header::From(orig_from)) = headers
        .iter()
        .find(|header| matches!(header, Header::From(_)))
    else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "missing From header",
        ));
    };
    let orig_from = orig_from.clone();
    headers.push(Header::To(orig_from.clone()));
    headers.retain(|header| !matches!(header, Header::From(_)));
    headers.push(Header::From(from.to_string()));
    // Set the In-Reply-To header to the original message ID
    headers.retain(|header| !matches!(header, Header::InReplyTo(_)));
    headers.retain(|header| !matches!(header, Header::References(_)));
    if let Some(Header::MessageID(orig_msg_id)) = orig_headers
        .iter()
        .find(|header| matches!(header, Header::MessageID(_)))
    {
        headers.push(Header::InReplyTo(orig_msg_id.clone()));
        headers.push(Header::References(orig_msg_id.clone()));
    }
    headers.retain(|header| !matches!(header, Header::MessageID(_)));
    headers.push(Header::MessageID(generate_message_id()));
    // Add Re: to the subject
    if let Some(Header::Subject(subject)) = headers
        .iter_mut()
        .find(|header| matches!(header, Header::Subject(_)))
    {
        if !subject.starts_with("Re: ") {
            *subject = format!("Re: {}", subject);
        }
    } else {
        headers.push(Header::Subject("Re: ".to_string()));
    }
    headers.sort_by(|a, b| {
        orig_headers
            .iter()
            .position(|x| Header::of_same_type(a, x))
            .cmp(&orig_headers.iter().position(|x| Header::of_same_type(b, x)))
    });
    let body = format!(
        "On {orig_date}, {orig_from} wrote:\n{}",
        body.split("\n")
            .map(|x| format!("> {}", x))
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(format!(
        "{}\n\n{}",
        headers
            .iter()
            .map(|header| match header {
                Header::From(x) => format!("From: {}", x),
                Header::To(x) => format!("To: {}", x),
                Header::Cc(x) => format!("Cc: {}", x),
                Header::Subject(x) => format!("Subject: {}", x),
                Header::Date(x) => format!("Date: {}", x),
                Header::MessageID(x) => format!("Message-ID: {}", x),
                Header::MimeVersion => "MIME-Version: 1.0".to_string(),
                // We accept ascii, we emit utf-8
                Header::ContentType => "Content-Type: text/plain; charset=utf-8".to_string(),
                Header::References(x) => format!("References: {}", x),
                Header::ContentDisposition => "Content-Disposition: inline".to_string(),
                Header::InReplyTo(x) => format!("In-Reply-To: {}", x),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        body
    ))
}

//////////////////////////////////////////// wrap_header ///////////////////////////////////////////

fn wrap_header(s: impl AsRef<str>) -> String {
    let mut s = s.as_ref().to_string();
    let mut wrapped = String::new();
    while s.len() > 70 {
        let split_at = s
            .chars()
            .enumerate()
            .filter(|(_, c)| *c == ' ')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(s.chars().count());
        let this_line = s.chars().take(split_at).collect::<String>();
        if wrapped.is_empty() {
            wrapped = this_line.trim().to_string();
        } else {
            wrapped.push_str(&format!("\n {}", this_line.trim()));
        }
        let remainder = s.chars().skip(split_at).collect::<String>();
        s = remainder.trim().to_string();
    }
    if !s.is_empty() {
        wrapped.push_str(&s);
    }
    wrapped
}

//////////////////////////////////////// generate_message_id ///////////////////////////////////////

/// Generate a message ID for the current host.
pub fn generate_message_id() -> String {
    format!(
        "<{}@{}>",
        chrono::Utc::now().timestamp(),
        std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".to_string())
    )
}
