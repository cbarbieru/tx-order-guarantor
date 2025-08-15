# Quick start
Below is an example to start `tx-order-guarantor` and invoke the 2 RPC endpoints: 
- send a transaction (alternative to casting via builder-playground [here](https://github.com/cbarbieru/builder-playground?tab=readme-ov-file#quick-start)) and 
- get the ordered list of transaction hashes
```bash
cargo run

curl -X POST -H "Content-Type: application/json" --data '{"method":"eth_sendRawTransaction","params":["0x02f86b0d01018201f78252089414dc79964da2c08b23698b3d3cc7ca32193d9955872386f26fc1000080c001a0fa3e843e1929b853a2d6e39c6b0ccc0f6472503fa446adf4c6b3aad6439d76b4a079ea207f5e7e6f6ee0b836ce81aa0bfd73cf69dd961a9834a2636ea4fbd3ec6b"],"id":4,"jsonrpc":"2.0"}' http://127.0.0.1:1545

curl -H "Content-Type: application/json" --data '{"method":"tog_getBestTransactionHashes","params":[],"id":4,"jsonrpc":"2.0"}' http://127.0.0.1:1545

```