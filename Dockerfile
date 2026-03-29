# Используем официальный образ Rust
FROM rust:1.75 as builder
WORKDIR /usr/src/mewchain
COPY . .
RUN cargo build --release

# Минимальный образ для запуска
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl-dev ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/src/mewchain/target/release/mewchain /usr/local/bin/mewchain
EXPOSE 8080
CMD ["mewchain"]