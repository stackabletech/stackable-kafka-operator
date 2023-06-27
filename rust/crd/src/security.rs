//! A helper module to process Apache Kafka security configuration
//!
//! This module merges the `tls` and `authentication` module and offers better accessibility
//! and helper functions
//!
//! This is required due to overlaps between TLS encryption and e.g. mTLS authentication or Kerberos

use crate::{
    authentication, authentication::ResolvedAuthenticationClasses, listener, tls, KafkaCluster,
    SERVER_PROPERTIES_FILE, STACKABLE_TMP_DIR,
};

use crate::listener::KafkaListenerConfig;
use snafu::{ResultExt, Snafu};
use stackable_operator::builder::SecretFormat;
use stackable_operator::{
    builder::{ContainerBuilder, PodBuilder, SecretOperatorVolumeSourceBuilder, VolumeBuilder},
    client::Client,
    commons::authentication::{AuthenticationClass, AuthenticationClassProvider},
};
use std::collections::BTreeMap;

#[derive(Snafu, Debug)]
pub enum Error {
    #[snafu(display("failed to process authentication class"))]
    InvalidAuthenticationClassConfiguration { source: authentication::Error },
}

/// Helper struct combining TLS settings for server and internal with the resolved AuthenticationClasses
pub struct KafkaTlsSecurity {
    resolved_authentication_classes: ResolvedAuthenticationClasses,
    internal_secret_class: String,
    server_secret_class: Option<String>,
}

impl KafkaTlsSecurity {
    // ports
    pub const CLIENT_PORT_NAME: &'static str = "kafka";
    pub const CLIENT_PORT: u16 = 9092;
    pub const SECURE_CLIENT_PORT_NAME: &'static str = "kafka-tls";
    pub const SECURE_CLIENT_PORT: u16 = 9093;
    pub const INTERNAL_PORT: u16 = 19092;
    pub const SECURE_INTERNAL_PORT: u16 = 19093;
    // - TLS global
    const SSL_STORE_PASSWORD: &'static str = "";
    // - TLS client
    const CLIENT_SSL_KEYSTORE_LOCATION: &'static str = "listener.name.client.ssl.keystore.location";
    const CLIENT_SSL_KEYSTORE_PASSWORD: &'static str = "listener.name.client.ssl.keystore.password";
    const CLIENT_SSL_KEYSTORE_TYPE: &'static str = "listener.name.client.ssl.keystore.type";
    const CLIENT_SSL_TRUSTSTORE_LOCATION: &'static str =
        "listener.name.client.ssl.truststore.location";
    const CLIENT_SSL_TRUSTSTORE_PASSWORD: &'static str =
        "listener.name.client.ssl.truststore.password";
    const CLIENT_SSL_TRUSTSTORE_TYPE: &'static str = "listener.name.client.ssl.truststore.type";
    // - TLS client authentication
    const CLIENT_AUTH_SSL_KEYSTORE_LOCATION: &'static str =
        "listener.name.client_auth.ssl.keystore.location";
    const CLIENT_AUTH_SSL_KEYSTORE_PASSWORD: &'static str =
        "listener.name.client_auth.ssl.keystore.password";
    const CLIENT_AUTH_SSL_KEYSTORE_TYPE: &'static str =
        "listener.name.client_auth.ssl.keystore.type";
    const CLIENT_AUTH_SSL_TRUSTSTORE_LOCATION: &'static str =
        "listener.name.client_auth.ssl.truststore.location";
    const CLIENT_AUTH_SSL_TRUSTSTORE_PASSWORD: &'static str =
        "listener.name.client_auth.ssl.truststore.password";
    const CLIENT_AUTH_SSL_TRUSTSTORE_TYPE: &'static str =
        "listener.name.client_auth.ssl.truststore.type";
    const CLIENT_AUTH_SSL_CLIENT_AUTH: &'static str = "listener.name.client_auth.ssl.client.auth";
    // - TLS internal
    const INTER_BROKER_LISTENER_NAME: &'static str = "inter.broker.listener.name";
    const INTER_SSL_KEYSTORE_LOCATION: &'static str =
        "listener.name.internal.ssl.keystore.location";
    const INTER_SSL_KEYSTORE_PASSWORD: &'static str =
        "listener.name.internal.ssl.keystore.password";
    const INTER_SSL_KEYSTORE_TYPE: &'static str = "listener.name.internal.ssl.keystore.type";
    const INTER_SSL_TRUSTSTORE_LOCATION: &'static str =
        "listener.name.internal.ssl.truststore.location";
    const INTER_SSL_TRUSTSTORE_PASSWORD: &'static str =
        "listener.name.internal.ssl.truststore.password";
    const INTER_SSL_TRUSTSTORE_TYPE: &'static str = "listener.name.internal.ssl.truststore.type";
    const INTER_SSL_CLIENT_AUTH: &'static str = "listener.name.internal.ssl.client.auth";
    // directories
    const STACKABLE_TLS_SERVER_PEM_DIR: &'static str = "/stackable/tls_server_pem";
    const STACKABLE_TLS_SERVER_PKCS12_DIR: &'static str = "/stackable/tls_server_pkcs12";
    const STACKABLE_TLS_INTERNAL_DIR: &'static str = "/stackable/tls_internal";

    pub fn new(
        resolved_authentication_classes: ResolvedAuthenticationClasses,
        internal_secret_class: String,
        server_secret_class: Option<String>,
    ) -> Self {
        Self {
            resolved_authentication_classes,
            internal_secret_class,
            server_secret_class,
        }
    }

    /// Create a `KafkaSecurity` struct from the Kafka custom resource and resolve
    /// all provided `AuthenticationClass` references.
    pub async fn new_from_kafka_cluster(
        client: &Client,
        kafka: &KafkaCluster,
    ) -> Result<Self, Error> {
        Ok(KafkaTlsSecurity {
            resolved_authentication_classes:
                authentication::ResolvedAuthenticationClasses::from_references(
                    client,
                    &kafka.spec.cluster_config.authentication,
                )
                .await
                .context(InvalidAuthenticationClassConfigurationSnafu)?,
            internal_secret_class: kafka
                .spec
                .cluster_config
                .tls
                .as_ref()
                .map(|tls| tls.internal_secret_class.clone())
                .unwrap_or_else(tls::internal_tls_default),
            server_secret_class: kafka
                .spec
                .cluster_config
                .tls
                .as_ref()
                .and_then(|tls| tls.server_secret_class.clone()),
        })
    }

    /// Check if TLS encryption is enabled. This could be due to:
    /// - A provided server `SecretClass`
    /// - A provided client `AuthenticationClass`
    /// This affects init container commands, Kafka configuration, volume mounts and
    /// the Kafka client port
    pub fn tls_enabled(&self) -> bool {
        // TODO: This must be adapted if other authentication methods are supported and require TLS
        self.tls_client_authentication_class().is_some() || self.tls_server_secret_class().is_some()
    }

    /// Retrieve an optional TLS secret class for external client -> server communications.
    pub fn tls_server_secret_class(&self) -> Option<&str> {
        self.server_secret_class.as_deref()
    }

    /// Retrieve an optional TLS `AuthenticationClass`.
    pub fn tls_client_authentication_class(&self) -> Option<&AuthenticationClass> {
        self.resolved_authentication_classes
            .get_tls_authentication_class()
    }

    /// Retrieve the mandatory internal `SecretClass`.
    pub fn tls_internal_secret_class(&self) -> Option<&str> {
        if !self.internal_secret_class.is_empty() {
            Some(self.internal_secret_class.as_str())
        } else {
            None
        }
    }

    /// Return the Kafka (secure) client port depending on tls or authentication settings.
    pub fn client_port(&self) -> u16 {
        if self.tls_enabled() {
            Self::SECURE_CLIENT_PORT
        } else {
            Self::CLIENT_PORT
        }
    }

    /// Return the Kafka (secure) client port name depending on tls or authentication settings.
    pub fn client_port_name(&self) -> &str {
        if self.tls_enabled() {
            Self::SECURE_CLIENT_PORT_NAME
        } else {
            Self::CLIENT_PORT_NAME
        }
    }

    /// Return the Kafka (secure) internal port depending on tls settings.
    pub fn internal_port(&self) -> u16 {
        if self.tls_internal_secret_class().is_some() {
            Self::SECURE_INTERNAL_PORT
        } else {
            Self::INTERNAL_PORT
        }
    }

    /// Returns SVC container command to retrieve the node port service port.
    pub fn svc_container_commands(&self) -> String {
        let port_name = self.client_port_name();
        // Extract the nodeport from the nodeport service
        format!("kubectl get service \"$POD_NAME\" -o jsonpath='{{.spec.ports[?(@.name==\"{port_name}\")].nodePort}}' | tee {STACKABLE_TMP_DIR}/{port_name}_nodeport")
    }

    /// Returns the commands for the kcat readiness probe.
    pub fn kcat_prober_container_commands(&self) -> Vec<String> {
        let mut args = vec!["/stackable/kcat".to_string()];
        let port = self.client_port();

        if self.tls_client_authentication_class().is_some() {
            args.push("-b".to_string());
            args.push(format!("localhost:{}", port));
            args.extend(Self::kcat_client_auth_ssl(
                Self::STACKABLE_TLS_SERVER_PEM_DIR,
            ));
        } else if self.tls_server_secret_class().is_some() {
            args.push("-b".to_string());
            args.push(format!("localhost:{}", port));
            args.extend(Self::kcat_client_ssl(Self::STACKABLE_TLS_SERVER_PEM_DIR));
        } else {
            args.push("-b".to_string());
            args.push(format!("localhost:{}", port));
        }

        args.push("-L".to_string());
        args
    }

    /// Returns the commands to start the main Kafka container
    pub fn kafka_container_commands(
        &self,
        kafka_listeners: &KafkaListenerConfig,
        opa_connect_string: Option<&str>,
    ) -> Vec<String> {
        vec![
            "bin/kafka-server-start.sh".to_string(),
            format!("/stackable/config/{}", SERVER_PROPERTIES_FILE),
            "--override \"zookeeper.connect=$ZOOKEEPER\"".to_string(),
            format!("--override \"listeners={}\"", kafka_listeners.listeners()),
            format!(
                "--override \"advertised.listeners={}\"",
                kafka_listeners.advertised_listeners()
            ),
            format!(
                "--override \"listener.security.protocol.map={}\"",
                kafka_listeners.listener_security_protocol_map()
            ),
            opa_connect_string.map_or("".to_string(), |opa| {
                format!("--override \"opa.authorizer.url={}\"", opa)
            }),
        ]
    }

    /// Adds required volumes and volume mounts to the pod and container builders
    /// depending on the tls and authentication settings.
    pub fn add_volume_and_volume_mounts(
        &self,
        pod_builder: &mut PodBuilder,
        cb_kcat_prober: &mut ContainerBuilder,
        cb_kafka: &mut ContainerBuilder,
    ) {
        // add tls (server or client authentication volumes) if required
        if let Some(tls_server_secret_class) = self.get_tls_secret_class() {
            // kcat requires pem files and not keystores
            cb_kcat_prober.add_volume_mount("server-tls-pem", Self::STACKABLE_TLS_SERVER_PEM_DIR);
            pod_builder.add_volume(
                VolumeBuilder::new("server-tls-pem")
                    .ephemeral(
                        SecretOperatorVolumeSourceBuilder::new(tls_server_secret_class)
                            .with_pod_scope()
                            .with_node_scope()
                            .with_format(SecretFormat::TlsPem)
                            .build(),
                    )
                    .build(),
            );

            // empty mount for trust and keystore
            cb_kafka.add_volume_mount("server-tls-pkcs12", Self::STACKABLE_TLS_SERVER_PKCS12_DIR);
            pod_builder.add_volume(
                VolumeBuilder::new("server-tls-pkcs12")
                    .ephemeral(
                        SecretOperatorVolumeSourceBuilder::new(tls_server_secret_class)
                            .with_pod_scope()
                            .with_node_scope()
                            .with_format(SecretFormat::TlsPkcs12)
                            .build(),
                    )
                    .build(),
            );
        }

        if let Some(tls_internal_secret_class) = self.tls_internal_secret_class() {
            cb_kafka.add_volume_mount("internal-tls", Self::STACKABLE_TLS_INTERNAL_DIR);
            pod_builder.add_volume(
                VolumeBuilder::new("internal-tls")
                    .ephemeral(
                        SecretOperatorVolumeSourceBuilder::new(tls_internal_secret_class)
                            .with_pod_scope()
                            .with_node_scope()
                            .with_format(SecretFormat::TlsPkcs12)
                            .build(),
                    )
                    .build(),
            );
        }
    }

    /// Returns required Kafka configuration settings for the `server.properties` file
    /// depending on the tls and authentication settings.
    pub fn config_settings(&self) -> BTreeMap<String, String> {
        let mut config = BTreeMap::new();

        // We set either client tls with authentication or client tls without authentication
        // If authentication is explicitly required we do not want to have any other CAs to
        // be trusted.
        if self.tls_client_authentication_class().is_some() {
            config.insert(
                Self::CLIENT_AUTH_SSL_KEYSTORE_LOCATION.to_string(),
                format!("{}/keystore.p12", Self::STACKABLE_TLS_SERVER_PKCS12_DIR),
            );
            config.insert(
                Self::CLIENT_AUTH_SSL_KEYSTORE_PASSWORD.to_string(),
                Self::SSL_STORE_PASSWORD.to_string(),
            );
            config.insert(
                Self::CLIENT_AUTH_SSL_KEYSTORE_TYPE.to_string(),
                "PKCS12".to_string(),
            );
            config.insert(
                Self::CLIENT_AUTH_SSL_TRUSTSTORE_LOCATION.to_string(),
                format!("{}/truststore.p12", Self::STACKABLE_TLS_SERVER_PKCS12_DIR),
            );
            config.insert(
                Self::CLIENT_AUTH_SSL_TRUSTSTORE_PASSWORD.to_string(),
                Self::SSL_STORE_PASSWORD.to_string(),
            );
            config.insert(
                Self::CLIENT_AUTH_SSL_TRUSTSTORE_TYPE.to_string(),
                "PKCS12".to_string(),
            );
            // client auth required
            config.insert(
                Self::CLIENT_AUTH_SSL_CLIENT_AUTH.to_string(),
                "required".to_string(),
            );
        } else if self.tls_server_secret_class().is_some() {
            config.insert(
                Self::CLIENT_SSL_KEYSTORE_LOCATION.to_string(),
                format!("{}/keystore.p12", Self::STACKABLE_TLS_SERVER_PKCS12_DIR),
            );
            config.insert(
                Self::CLIENT_SSL_KEYSTORE_PASSWORD.to_string(),
                Self::SSL_STORE_PASSWORD.to_string(),
            );
            config.insert(
                Self::CLIENT_SSL_KEYSTORE_TYPE.to_string(),
                "PKCS12".to_string(),
            );
            config.insert(
                Self::CLIENT_SSL_TRUSTSTORE_LOCATION.to_string(),
                format!("{}/truststore.p12", Self::STACKABLE_TLS_SERVER_PKCS12_DIR),
            );
            config.insert(
                Self::CLIENT_SSL_TRUSTSTORE_PASSWORD.to_string(),
                Self::SSL_STORE_PASSWORD.to_string(),
            );
            config.insert(
                Self::CLIENT_SSL_TRUSTSTORE_TYPE.to_string(),
                "PKCS12".to_string(),
            );
        }

        // Internal TLS
        if self.tls_internal_secret_class().is_some() {
            config.insert(
                Self::INTER_SSL_KEYSTORE_LOCATION.to_string(),
                format!("{}/keystore.p12", Self::STACKABLE_TLS_INTERNAL_DIR),
            );
            config.insert(
                Self::INTER_SSL_KEYSTORE_PASSWORD.to_string(),
                Self::SSL_STORE_PASSWORD.to_string(),
            );
            config.insert(
                Self::INTER_SSL_KEYSTORE_TYPE.to_string(),
                "PKCS12".to_string(),
            );
            config.insert(
                Self::INTER_SSL_TRUSTSTORE_LOCATION.to_string(),
                format!("{}/truststore.p12", Self::STACKABLE_TLS_INTERNAL_DIR),
            );
            config.insert(
                Self::INTER_SSL_TRUSTSTORE_PASSWORD.to_string(),
                Self::SSL_STORE_PASSWORD.to_string(),
            );
            config.insert(
                Self::INTER_SSL_TRUSTSTORE_TYPE.to_string(),
                "PKCS12".to_string(),
            );
            config.insert(
                Self::INTER_SSL_CLIENT_AUTH.to_string(),
                "required".to_string(),
            );
        }

        // common
        config.insert(
            Self::INTER_BROKER_LISTENER_NAME.to_string(),
            listener::KafkaListenerName::Internal.to_string(),
        );

        config
    }

    /// Returns the `SecretClass` provided in a `AuthenticationClass` for TLS.
    fn get_tls_secret_class(&self) -> Option<&String> {
        self.resolved_authentication_classes
            .get_tls_authentication_class()
            .and_then(|auth_class| match &auth_class.spec.provider {
                AuthenticationClassProvider::Tls(tls) => tls.client_cert_secret_class.as_ref(),
                _ => None,
            })
            .or(self.server_secret_class.as_ref())
    }

    fn kcat_client_auth_ssl(cert_directory: &str) -> Vec<String> {
        vec![
            "-X".to_string(),
            "security.protocol=SSL".to_string(),
            "-X".to_string(),
            format!("ssl.key.location={cert_directory}/tls.key"),
            "-X".to_string(),
            format!("ssl.certificate.location={cert_directory}/tls.crt"),
            "-X".to_string(),
            format!("ssl.ca.location={cert_directory}/ca.crt"),
        ]
    }

    fn kcat_client_ssl(cert_directory: &str) -> Vec<String> {
        vec![
            "-X".to_string(),
            "security.protocol=SSL".to_string(),
            "-X".to_string(),
            format!("ssl.ca.location={cert_directory}/ca.crt"),
        ]
    }
}
