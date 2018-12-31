FROM rust:1.23.0

WORKDIR /src
COPY . .

RUN cargo install

CMD ["websocket_server"]