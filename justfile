# Variables
container := "esp32-dev"
image := "planty-dev"
port := "/dev/ttyUSB0"
wifi_ssid := env_var_or_default("WIFI_SSID", "CHANGE_ME")
wifi_pass := env_var_or_default("WIFI_PASSWORD", "CHANGE_ME")

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

up-dev: build-image
    @if [ -n "$(docker ps -a -q -f name={{ container }})" ]; then \
        docker rm -f {{ container }} > /dev/null 2>&1; \
    fi
    docker run -d --name {{ container }} -v $(pwd):$(pwd) -w $(pwd) {{ image }}
    @echo "🚀 Conteneur {{ container }} prêt pour Neovim et Rust !"

down:
    -docker stop {{ container }}
    -docker rm {{ container }}

build:
    docker exec {{ container }} env WIFI_SSID={{ wifi_ssid }} WIFI_PASSWORD={{ wifi_pass }} cargo build

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
