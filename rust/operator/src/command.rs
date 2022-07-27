use stackable_kafka_crd::{
    KafkaCluster, CLIENT_PORT, SECURE_CLIENT_PORT, SSL_KEYSTORE_LOCATION, SSL_KEYSTORE_PASSWORD,
    SSL_STORE_PASSWORD, SSL_TRUSTSTORE_LOCATION, SSL_TRUSTSTORE_PASSWORD, STACKABLE_DATA_DIR,
    STACKABLE_TLS_CERTS_DIR, STACKABLE_TMP_DIR, SYSTEM_TRUST_STORE_DIR,
};

pub fn prepare_container_cmd_args(kafka: &KafkaCluster) -> String {
    let mut args = vec![
        // Copy system truststore to stackable truststore
        format!("keytool -importkeystore -srckeystore {SYSTEM_TRUST_STORE_DIR} -srcstoretype jks -srcstorepass {SSL_STORE_PASSWORD} -destkeystore {STACKABLE_TLS_CERTS_DIR}/truststore.p12 -deststoretype pkcs12 -deststorepass {SSL_STORE_PASSWORD} -noprompt")
    ];

    if kafka.client_tls_secret_class().is_some() {
        args.extend(create_key_and_trust_store(
            STACKABLE_TLS_CERTS_DIR,
            "stackable-ca-cert",
        ));
        args.extend(chown_and_chmod(STACKABLE_TLS_CERTS_DIR));
    }

    args.extend(chown_and_chmod(STACKABLE_DATA_DIR));
    args.extend(chown_and_chmod(STACKABLE_TMP_DIR));

    args.join(" && ")
}

pub fn kcat_container_cmd_args(kafka: &KafkaCluster) -> Vec<String> {
    let mut args = vec!["kcat".to_string()];

    if kafka.client_tls_secret_class().is_some() {
        args.push("-b".to_string());
        args.push(format!("localhost:{}", SECURE_CLIENT_PORT));
        args.extend([
            "-X".to_string(),
            "security.protocol=SSL".to_string(),
            "-X".to_string(),
            format!(
                "{}={}/keystore.p12",
                SSL_KEYSTORE_LOCATION, STACKABLE_TLS_CERTS_DIR
            ),
            "-X".to_string(),
            format!("{}={}", SSL_KEYSTORE_PASSWORD, SSL_STORE_PASSWORD),
            "-X".to_string(),
            format!(
                "{}={}/truststore.p12",
                SSL_TRUSTSTORE_LOCATION, STACKABLE_TLS_CERTS_DIR
            ),
            "-X".to_string(),
            format!("{}={}", SSL_TRUSTSTORE_PASSWORD, SSL_STORE_PASSWORD),
        ]);
    } else {
        args.push("-b".to_string());
        args.push(format!("localhost:{}", CLIENT_PORT));
    }

    args.push("-L".to_string());

    args
}

/// Generates the shell script to create key and truststores from the certificates provided
/// by the secret operator.
fn create_key_and_trust_store(directory: &str, alias_name: &str) -> Vec<String> {
    vec![
        format!("echo [{dir}] Creating truststore", dir = directory),
        format!("keytool -importcert -file {dir}/ca.crt -keystore {dir}/truststore.p12 -storetype pkcs12 -noprompt -alias {alias} -storepass {password}",
                dir = directory, alias = alias_name, password = SSL_STORE_PASSWORD),
        format!("echo [{dir}] Creating certificate chain", dir = directory),
        format!("cat {dir}/ca.crt {dir}/tls.crt > {dir}/chain.crt", dir = directory),
        format!("echo [{dir}] Creating keystore", dir = directory),
        format!("openssl pkcs12 -export -in {dir}/chain.crt -inkey {dir}/tls.key -out {dir}/keystore.p12 --passout pass:{password}",
                dir = directory, password = SSL_STORE_PASSWORD),
    ]
}

/// Generates a shell script to chown and chmod the provided directory.
fn chown_and_chmod(directory: &str) -> Vec<String> {
    vec![
        format!("echo chown and chmod {dir}", dir = directory),
        format!("chown -R stackable:stackable {dir}", dir = directory),
        format!("chmod -R a=,u=rwX {dir}", dir = directory),
    ]
}
