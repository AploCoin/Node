# Installation node from source to Alpine 

1. Update list of packages and install rust, cargo and git
```
apk update
apk install git rust cargo
```
2. Clone this repo
```
git clone https://github.com/AploCoin/Node -b dev node
```
3. Edit  .env file.  Replace IP address in the ANNOUNCE_ADDRESS field to yours
```
ANNOUNCE_ADDRESS="yourIP:5050"
```
4. Build node
 ```
cargo build --release
```
5. Run node
```
cargo run --release
```
