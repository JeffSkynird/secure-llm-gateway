FROM rust:1.80 as builder
WORKDIR /app
COPY Cargo.toml .
RUN mkdir src && echo 'fn main(){}' > src/main.rs && cargo build --release
COPY src ./src
RUN cargo build --release

FROM gcr.io/distroless/cc-debian12
COPY --from=builder /app/target/release/secure-llm-gateway /bin/secure-llm-gateway
ENV RUST_LOG=info
EXPOSE 8080
ENTRYPOINT ["/bin/secure-llm-gateway"]
