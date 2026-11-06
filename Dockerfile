FROM rust:1.99.1-trixie AS builder
WORKDIR /usr/src/asksms

# Compile dependencies with a stub main.rs so they can be cached
RUN mkdir src
RUN echo 'fn main() { dbg!("stub"); }' > src/main.rs
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release
RUN rm -rf src

# Build the actual project
COPY . ./
RUN touch src/main.rs
RUN cargo build --release --bin asksms

# Debian version must match the builder's, since the binary links against glibc.
# Includes the CA certificates that reqwest's TLS verifier reads at runtime.
FROM gcr.io/distroless/cc-debian13
COPY --from=builder /usr/src/asksms/target/release/asksms /usr/local/bin/asksms
CMD ["/usr/local/bin/asksms"]
