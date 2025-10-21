# Quick start
Below is an example to start `tx-order-guarantor` and invoke the 2 RPC endpoints: 
- send a transaction (alternative to casting via builder-playground [here](https://github.com/cbarbieru/builder-playground?tab=readme-ov-file#quick-start)) and 
- get the ordered list of transaction hashes
```bash
cargo run

curl -X POST -H "Content-Type: application/json" --data '{"method":"eth_sendRawTransaction","params":["0x02f86b0d01018201f78252089414dc79964da2c08b23698b3d3cc7ca32193d9955872386f26fc1000080c001a0fa3e843e1929b853a2d6e39c6b0ccc0f6472503fa446adf4c6b3aad6439d76b4a079ea207f5e7e6f6ee0b836ce81aa0bfd73cf69dd961a9834a2636ea4fbd3ec6b"],"id":4,"jsonrpc":"2.0"}' http://127.0.0.1:1545

curl -H "Content-Type: application/json" --data '{"method":"tog_getBestTransactionHashes","params":[],"id":4,"jsonrpc":"2.0"}' http://127.0.0.1:1545

cast send 0x70997970C51812dc3A010C7d01b50e0d17dc79C8 \
  --value 0.01ether \
  --private-key 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80 \
  --rpc-url http://localhost:1545

cast tx 0xd91aca3a8ff1cab0673ce53d6938e977ba2832a7d9a2d60c56ecc5b1e767e5df --rpc-url http://localhost:8547

cast block 65  --rpc-url http://localhost:8547

$ docker run -p 80:80 -e APP_NODE_URL="http://localhost:8545" alethio/ethereum-lite-explorer

```