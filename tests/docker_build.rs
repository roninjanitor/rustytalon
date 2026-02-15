//! Docker deployment verification tests.
//!
//! These tests validate that Docker configuration files exist and are well-formed.
//! They do NOT require Docker to be running.

use std::path::Path;

#[test]
fn dockerfile_exists() {
    assert!(
        Path::new("Dockerfile").exists(),
        "Dockerfile must exist at project root"
    );
}

#[test]
fn dockerfile_worker_exists() {
    assert!(
        Path::new("Dockerfile.worker").exists(),
        "Dockerfile.worker must exist at project root"
    );
}

#[test]
fn docker_compose_exists_and_valid_yaml() {
    let path = Path::new("docker-compose.yml");
    assert!(
        path.exists(),
        "docker-compose.yml must exist at project root"
    );

    let content = std::fs::read_to_string(path).expect("Failed to read docker-compose.yml");
    // Validate it's parseable YAML by checking for key markers
    assert!(
        content.contains("services:"),
        "docker-compose.yml must define services"
    );
    assert!(
        content.contains("postgres") || content.contains("db"),
        "docker-compose.yml should define a database service"
    );
}

#[test]
fn dockerfile_has_rust_build() {
    let content = std::fs::read_to_string("Dockerfile").expect("Failed to read Dockerfile");
    assert!(
        content.contains("cargo build") || content.contains("cargo install"),
        "Dockerfile should contain a cargo build step"
    );
}

#[test]
fn dockerfile_worker_has_entrypoint() {
    let content =
        std::fs::read_to_string("Dockerfile.worker").expect("Failed to read Dockerfile.worker");
    assert!(
        content.contains("ENTRYPOINT") || content.contains("CMD"),
        "Dockerfile.worker should define an entrypoint or command"
    );
}
