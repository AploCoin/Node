# Node
*  [Switch to English](https://github.com/AploCoin/Node/blob/dev/README.md)
## Installation
Есть 2 способа установки: 
- В Docker (Протестировано на: Ubuntu 22.04)
- Из исходного кода (Протестировано на: Ubuntu 18.04, Debian 12, Termux 0.118.0) 

### Docker
1. Обновите список пакетов и установите некоторые необходимые пакеты
```
sudo apt update
sudo apt install -y nano mc git curl
```
2. Установите Докер

Актуальные инструкции [по установке](https://docs.docker.com/engine/install/ubuntu/) Docker

3. Клонируйте этот репозиторий

```
git clone https://github.com/AploCoin/Node -b dev node
```
4. Отредактируйте файл .env   Замените IP адрес в поле ANNOUNCE_ADDRESS на ваш
```
ANNOUNCE_ADDRESS="yourIP:5050"
```
5. Соберите контейнер Docker
```
docker build -t aplo_node:latest .
```
6. Запустите контейнер
```
mkdir logs
docker run --network host --name aplo_node -v $(pwd)/logs:/Node/logs aplo_node
```

### Из исходного кода
1. Обновите список пакетов и установите некоторые необходимые пакеты
```
sudo apt update
sudo apt install -y curl git nano build-essential binutils
```
2. Установите Rust
```
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```
3. Клонируйте этот репозиторий

```
git clone https://github.com/AploCoin/Node -b dev node
```
4. Отредактируйте файл .env   Замените IP адрес в поле ANNOUNCE_ADDRESS на ваш
```
ANNOUNCE_ADDRESS="yourIP:5050"
```
5. Соберите ноду
```
cargo build --release
```
6. Запустите ноду
```
cargo run --release
```
