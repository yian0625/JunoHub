use std::time::Duration;

use lettre::{
    message::{Mailbox, MultiPart, SinglePart},
    transport::smtp::{authentication::Credentials, AsyncSmtpTransportBuilder, Error as SmtpError},
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
};

use crate::{
    config::{SmtpConfig, SmtpSecurity},
    email_verification::email_content,
    error::{AppError, AppResult},
};

fn unavailable(message: &str) -> AppError {
    AppError::ServiceUnavailable(message.to_owned())
}

pub fn validate(config: &SmtpConfig) -> AppResult<()> {
    for (value, name) in [
        (&config.host, "SMTP_HOST"),
        (&config.username, "SMTP_USERNAME"),
        (&config.password, "SMTP_PASSWORD"),
        (&config.from, "SMTP_FROM"),
    ] {
        if value.trim().is_empty() {
            return Err(unavailable(&format!(
                "验证码邮件服务缺少 {name}，请联系管理员配置。"
            )));
        }
    }
    if config.host.contains(['/', ':', '@']) || config.host.chars().any(char::is_whitespace) {
        return Err(unavailable(
            "SMTP_HOST 格式不正确，请填写服务器主机名，不要包含协议、端口或账号。",
        ));
    }
    config
        .from
        .parse::<Mailbox>()
        .map_err(|_| unavailable("SMTP_FROM 发件地址格式不正确，请联系管理员检查配置。"))?;
    Ok(())
}

fn transport_builder(config: &SmtpConfig) -> AppResult<AsyncSmtpTransportBuilder> {
    validate(config)?;
    let builder = match config.security {
        SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host),
        SmtpSecurity::StartTls => {
            AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)
        }
    }
    .map_err(|_| unavailable("SMTP TLS 配置无效，请联系管理员检查服务器地址。"))?;
    Ok(builder
        .port(config.port)
        .credentials(Credentials::new(
            config.username.clone(),
            config.password.clone(),
        ))
        .timeout(Some(Duration::from_secs(15))))
}

fn message(config: &SmtpConfig, email: &str, code: &str, generation: &str) -> AppResult<Message> {
    let from = config
        .from
        .parse::<Mailbox>()
        .map_err(|_| unavailable("SMTP_FROM 发件地址格式不正确，请联系管理员检查配置。"))?;
    let recipient = email.parse::<Mailbox>().map_err(|_| AppError::FormFailed {
        reason: "email_invalid",
        message: "邮箱地址格式不正确。",
        field: "email",
    })?;
    let message_id_domain = from.email.domain().to_owned();
    let (text, html) = email_content(code);
    Message::builder()
        .from(from)
        .to(recipient)
        .subject("JunoHub 注册验证码")
        .message_id(Some(format!(
            "<registration.{generation}@{message_id_domain}>"
        )))
        .multipart(
            MultiPart::alternative()
                .singlepart(SinglePart::plain(text))
                .singlepart(SinglePart::html(html)),
        )
        .map_err(|_| unavailable("验证码邮件构建失败，请联系管理员检查发件配置。"))
}

pub async fn deliver(
    config: &SmtpConfig,
    email: &str,
    code: &str,
    generation: &str,
) -> AppResult<()> {
    let mailer = transport_builder(config)?.build();
    transmit(&mailer, message(config, email, code, generation)?).await
}

async fn transmit(mailer: &AsyncSmtpTransport<Tokio1Executor>, message: Message) -> AppResult<()> {
    // Bound the entire exchange, including DNS, TLS, authentication and DATA. Do not retry
    // ambiguous deliveries automatically: the server may have already accepted the message.
    let response = tokio::time::timeout(Duration::from_secs(20), mailer.send(message))
        .await
        .map_err(|_| unavailable("SMTP 邮件发送超时，请稍后重试。"))?
        .map_err(delivery_error)?;
    if !response.is_positive() {
        return Err(unavailable("SMTP 服务器未接受验证码邮件，请稍后重试。"));
    }
    Ok(())
}

fn delivery_error(error: SmtpError) -> AppError {
    let status = error.status().map(|code| code.to_string());
    let tls = is_tls_error(&error);
    tracing::warn!(
        status = status.as_deref().unwrap_or("none"),
        tls,
        timeout = error.is_timeout(),
        "registration email rejected by SMTP"
    );
    if error.is_timeout() {
        return unavailable("SMTP 邮件发送超时，请稍后重试。");
    }
    if tls {
        return unavailable("SMTP 加密连接失败，请联系管理员检查端口、TLS 模式和服务器证书。");
    }
    match status.as_deref() {
        Some("530" | "534" | "535" | "538") => {
            unavailable("SMTP 认证失败，请联系管理员检查邮箱账号、授权码和 SMTP 服务是否已开启。")
        }
        Some("550" | "551" | "553") => unavailable(
            "SMTP 拒绝发件人或收件人，请联系管理员检查发件地址与邮箱权限，并确认收件地址有效。",
        ),
        Some("552") => unavailable("SMTP 邮箱容量或发送额度不足，请稍后重试或联系管理员。"),
        _ if error.is_transient() => {
            unavailable("SMTP 服务器暂时拒绝发送，可能触发频率或额度限制，请稍后重试。")
        }
        _ if error.is_permanent() => {
            unavailable("SMTP 服务器拒绝此邮件，请联系管理员检查邮箱发送权限。")
        }
        _ => unavailable("SMTP 连接失败，请联系管理员检查服务器地址、端口和网络连接。"),
    }
}

fn is_tls_error(error: &SmtpError) -> bool {
    use std::error::Error;
    if error.is_tls() {
        return true;
    }
    let mut source = error.source();
    while let Some(cause) = source {
        if cause.is::<rustls::Error>()
            || cause
                .downcast_ref::<std::io::Error>()
                .and_then(|io| io.get_ref())
                .is_some_and(|inner| inner.is::<rustls::Error>())
        {
            return true;
        }
        source = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use lettre::transport::smtp::client::{Certificate, Tls, TlsParameters};
    use tokio::{
        io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
        net::TcpListener,
    };
    use tokio_rustls::{
        rustls::{self, pki_types::PrivatePkcs8KeyDer},
        TlsAcceptor,
    };

    fn fixture_config() -> SmtpConfig {
        SmtpConfig {
            host: "localhost".into(),
            username: "fixture@example.test".into(),
            password: "fixture-secret".into(),
            from: "JunoHub <fixture@example.test>".into(),
            ..Default::default()
        }
    }

    async fn reply<S: AsyncWrite + Unpin>(stream: &mut S, text: &str) {
        stream.write_all(text.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();
    }

    async fn dialogue<S: AsyncRead + AsyncWrite + Unpin>(
        stream: S,
        rejection: Option<&str>,
        greeting: bool,
    ) -> String {
        let mut stream = BufReader::new(stream);
        if greeting {
            reply(stream.get_mut(), "220 localhost fixture\r\n").await;
        }
        let mut line = String::new();
        let mut data = String::new();
        loop {
            line.clear();
            if stream.read_line(&mut line).await.unwrap() == 0 {
                break;
            }
            if line.starts_with("EHLO ") {
                reply(stream.get_mut(), "250-localhost\r\n250 AUTH PLAIN\r\n").await;
            } else if let Some(encoded) = line.strip_prefix("AUTH PLAIN ") {
                assert_eq!(
                    STANDARD.decode(encoded.trim()).unwrap(),
                    b"\0fixture@example.test\0fixture-secret"
                );
                if rejection == Some("auth") {
                    reply(
                        stream.get_mut(),
                        "535 5.7.8 private fixture-secret bad auth\r\n",
                    )
                    .await;
                    return data;
                } else {
                    reply(stream.get_mut(), "235 2.7.0 authenticated\r\n").await;
                }
            } else if line.starts_with("MAIL FROM:") {
                assert!(line.contains("<fixture@example.test>"));
                reply(stream.get_mut(), "250 sender ok\r\n").await;
            } else if line.starts_with("RCPT TO:") {
                assert!(line.contains("<member@example.test>"));
                reply(stream.get_mut(), "250 recipient ok\r\n").await;
            } else if line == "DATA\r\n" {
                reply(stream.get_mut(), "354 send content\r\n").await;
                loop {
                    line.clear();
                    assert_ne!(stream.read_line(&mut line).await.unwrap(), 0);
                    if line == ".\r\n" {
                        break;
                    }
                    data.push_str(&line);
                }
                reply(
                    stream.get_mut(),
                    if rejection == Some("data") {
                        "554 private response rejects message\r\n"
                    } else {
                        "250 2.0.0 queued\r\n"
                    },
                )
                .await;
                if rejection == Some("data") {
                    return data;
                }
            } else if line == "QUIT\r\n" {
                reply(stream.get_mut(), "221 closing\r\n").await;
                break;
            } else {
                panic!("unexpected SMTP command");
            }
        }
        data
    }

    async fn fixture(
        security: SmtpSecurity,
        rejection: Option<&'static str>,
    ) -> (SmtpConfig, Certificate, tokio::task::JoinHandle<String>) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let trusted = Certificate::from_der(cert.cert.der().to_vec()).unwrap();
        let server = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.cert.der().clone()],
            PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into(),
        )
        .unwrap();
        let acceptor = TlsAcceptor::from(std::sync::Arc::new(server));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = SmtpConfig {
            port: listener.local_addr().unwrap().port(),
            security,
            ..fixture_config()
        };
        let server = tokio::spawn(async move {
            let (mut tcp, _) = listener.accept().await.unwrap();
            if security == SmtpSecurity::StartTls {
                let mut stream = BufReader::new(tcp);
                reply(stream.get_mut(), "220 localhost fixture\r\n").await;
                let mut line = String::new();
                stream.read_line(&mut line).await.unwrap();
                assert!(line.starts_with("EHLO "));
                reply(stream.get_mut(), "250-localhost\r\n250 STARTTLS\r\n").await;
                line.clear();
                stream.read_line(&mut line).await.unwrap();
                assert_eq!(line, "STARTTLS\r\n");
                reply(stream.get_mut(), "220 upgrade\r\n").await;
                tcp = stream.into_inner();
            }
            match acceptor.accept(tcp).await {
                Ok(stream) => {
                    // STARTTLS does not send a second greeting after upgrading.
                    dialogue(stream, rejection, security == SmtpSecurity::Tls).await
                }
                Err(_) => String::new(),
            }
        });
        (config, trusted, server)
    }

    fn trusted_transport(
        config: &SmtpConfig,
        cert: Certificate,
    ) -> AsyncSmtpTransport<Tokio1Executor> {
        let parameters = TlsParameters::builder(config.host.clone())
            .add_root_certificate(cert)
            .build()
            .unwrap();
        let tls = match config.security {
            SmtpSecurity::Tls => Tls::Wrapper(parameters),
            SmtpSecurity::StartTls => Tls::Required(parameters),
        };
        transport_builder(config).unwrap().tls(tls).build()
    }

    #[tokio::test]
    async fn smtp_sends_authenticated_multipart_over_tls_and_starttls() {
        for security in [SmtpSecurity::Tls, SmtpSecurity::StartTls] {
            let (config, cert, server) = fixture(security, None).await;
            let mailer = trusted_transport(&config, cert);
            transmit(
                &mailer,
                message(
                    &config,
                    "member@example.test",
                    "123456",
                    "fixture-generation",
                )
                .unwrap(),
            )
            .await
            .unwrap();
            let data = tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
            assert!(data.contains("multipart/alternative"));
            assert!(data.contains("text/plain"));
            assert!(data.contains("text/html"));
            assert!(data.contains("123456"));
            assert!(data.contains("Message-ID: <registration.fixture-generation@example.test>"));
        }
    }

    #[tokio::test]
    async fn smtp_rejections_are_not_success_and_never_expose_provider_text() {
        for (rejection, expected) in [("auth", "认证失败"), ("data", "服务器拒绝此邮件")]
        {
            let (config, cert, server) = fixture(SmtpSecurity::Tls, Some(rejection)).await;
            let error = transmit(
                &trusted_transport(&config, cert),
                message(&config, "member@example.test", "123456", "fixture").unwrap(),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains(expected));
            assert!(!error.to_string().contains("fixture-secret"));
            assert!(!error.to_string().contains("private response"));
            tokio::time::timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[tokio::test]
    async fn smtp_rejects_untrusted_certificates_and_starttls_downgrades() {
        let (config, _, server) = fixture(SmtpSecurity::Tls, None).await;
        let error = deliver(&config, "member@example.test", "123456", "fixture")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("加密连接失败"));
        assert!(server.await.unwrap().is_empty());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = SmtpConfig {
            port: listener.local_addr().unwrap().port(),
            security: SmtpSecurity::StartTls,
            ..fixture_config()
        };
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            reply(stream.get_mut(), "220 localhost fixture\r\n").await;
            let mut line = String::new();
            stream.read_line(&mut line).await.unwrap();
            assert!(line.starts_with("EHLO "));
            reply(stream.get_mut(), "250-localhost\r\n250 AUTH PLAIN\r\n").await;
            line.clear();
            stream.read_line(&mut line).await.unwrap();
            assert!(!line.starts_with("AUTH"));
        });
        assert!(deliver(&config, "member@example.test", "123456", "fixture")
            .await
            .is_err());
        server.await.unwrap();
    }

    #[test]
    fn smtp_missing_configuration_and_bad_from_are_actionable() {
        let mut config = fixture_config();
        config.password.clear();
        assert!(validate(&config)
            .unwrap_err()
            .to_string()
            .contains("SMTP_PASSWORD"));
        config.password = "fixture-secret".into();
        config.from = "not an email".into();
        assert!(validate(&config)
            .unwrap_err()
            .to_string()
            .contains("SMTP_FROM"));
        assert!(!format!("{config:?}").contains("fixture-secret"));
    }
}
