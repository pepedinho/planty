FROM espressif/idf-rust:esp32_latest

WORKDIR /project

RUN rustup toolchain install stable --profile minimal -c rust-analyzer

CMD ["tail", "-f", "/dev/null"]
