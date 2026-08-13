# Variables
container := "esp32-dev"
image := "planty-dev"
port := "/dev/ttyUSB0"

default:
    @just --list

build-image:
    docker build -t {{ image }} .

up: build-image
    @if [ -n "$(docker ps -a -q -f name={{ container }})" ]; then \
        docker rm -f {{ container }} > /dev/null 2>&1; \
    fi
    docker run -d \
        --name {{ container }} \
        --device={{ port }} \
        -v $(pwd):$(pwd) \
        -w $(pwd) \
        {{ image }}
    @echo "🚀 Conteneur {{ container }} prêt pour Neovim et Rust !"

down:
    -docker stop {{ container }}
    -docker rm {{ container }}

build:
    docker exec -it {{ container }} cargo build

flash:
    espflash flash --port /dev/ttyUSB0 --monitor target/xtensa-esp32-none-elf/debug/planty

run:
    docker exec -it {{ container }} cargo run

monitor:
    docker exec -it {{ container }} espflash monitor {{ port }}

shell:
    docker exec -it {{ container }} /bin/bash

clean:
    docker exec -it {{ container }} cargo clean
