FROM rust:latest as builder
WORKDIR /Node
COPY . .

RUN cargo build --release

EXPOSE 5050

CMD ["./target/release/node"]
