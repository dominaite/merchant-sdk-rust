fn main() {
    let body = std::fs::read_to_string(std::env::var("BODY_FILE").unwrap()).unwrap();
    let sig = std::env::var("SIG").unwrap();
    let secret = std::env::var("DOMINAITE_WEBHOOK_SECRET").unwrap();
    dominaite::verify_webhook(&body, &sig, &secret, 300, None).expect("verification failed");
}
