use clap::{Parser, ValueHint};
use ethers::{
    abi::Abi,
    solc::{info::ContractInfo, Project},
    types::Bytes,
};
use eyre::Context;
/// ZKSync Contract Deployment Module
/// This module encapsulates the logic required for contract deployment, including:
/// - Retrieving the contract bytecode and ABI from the Solidity project
/// - Encoding the constructor arguments
/// - Signing the deployment transaction
/// - Handling the deployment process
///
/// This module plays a crucial role in the zkSync ecosystem by enabling developers to
/// seamlessly deploy and interact with zkSync contracts.
///
/// The main struct in this module is `ZkCreateArgs`, which represents the command-line
/// arguments for the `forge zk-create` command. It contains fields such as:
/// - The contract identifier
/// - Constructor arguments
/// - Transaction options
/// - Ethereum-specific options
///
/// The `ZkCreateArgs` struct implements methods to:
/// - Execute the deployment process
/// - Deploy the contract on the Ethereum network
///
/// Additionally, this module provides several helper functions to assist with the contract
/// deployment, including:
/// - Retrieving the bytecode and ABI of the contract from the Solidity project
/// - Parsing and encoding the constructor arguments
/// - Creating a signer for transaction signing
/// - Handling factory dependencies, if any
///
/// The contract deployment process involves:
/// 1. Setting up the project
/// 2. Retrieving the contract bytecode and ABI
/// 3. Parsing and encoding the constructor arguments
/// 4. Creating a signer with the provided private key and chain information
/// 5. Initializing a wallet for deployment
/// 6. Sending the deployment transaction to the Ethereum network
/// 7. Printing contract address, transaction hash, gas used, gas price, and block number if
///    the deployment is successful
///
/// To use the `forge zk-create` command:
/// 1. Parse the command-line arguments using the `ZkCreateArgs::parse()` method
/// 2. Execute the deployment process by calling the `run()` method on the parsed arguments
///
/// It's worth noting that this module relies on the following crates for interacting with
/// Ethereum and zkSync:
/// - `ethers`
/// - `zksync`
use foundry_cli::{
    opts::{CoreBuildArgs, EthereumOpts, TransactionOpts},
    utils::read_constructor_args_file,
};
use foundry_common::zk_utils::{get_chain, get_private_key, get_rpc_url};
use foundry_config::Config;
use serde_json::Value;
use std::{fs, path::PathBuf, str::FromStr};
use zksync_web3_rs::{
    providers::Provider,
    signers::{LocalWallet, Signer},
    ZKSWallet,
};

/// CLI arguments for `forge zk-create`.
/// Struct `ZkCreateArgs` encapsulates the arguments necessary for creating a new zkSync contract.
///
/// This struct is used to cleanly pass the required parameters for contract deployment to the
/// `create` function. It ensures type safety and reduces the chance of passing incorrect or
/// mismatched parameters.
///
/// The `ZkCreateArgs` struct has the following fields:
///
/// * `constructor_args`: This field represents the arguments for the zkSync contract constructor.
///   The arguments are represented as a vector of `Token` values. Each `Token` corresponds to an
///   argument of the contract's constructor.
///
/// * `encoded_constructor_args`: This is the hex encoded string representation of the constructor
///   arguments. It is used when deploying the contract on the Ethereum network.
///
/// * `bytecode`: The bytecode of the zkSync contract that is to be deployed. This is a compiled
///   version of the contract's source code.
///
/// * `private_key`: The private key used for signing the contract deployment transaction. It is the
///   private key of the account that will own the deployed contract.
///
/// * `chain_id`: The ID of the Ethereum network chain where the contract is to be deployed.
///   Different chain IDs represent different Ethereum networks (e.g., mainnet, testnet).
#[derive(Debug, Clone, Parser)]
#[clap(next_help_heading = "ZkCreate options", about = None)]
pub struct ZkCreateArgs {
    /// The contract identifier in the form `<path>:<contractname>`.
    #[clap(
        help = "The contract identifier in the form `<path>:<contractname>`.",
        value_name = "CONTRACT"
    )]
    contract: ContractInfo,

    /// The constructor arguments.
    #[clap(
        long,
        num_args(1..),
        help = "The constructor arguments.",
        name = "constructor_args",
        conflicts_with = "constructor_args_path",
        value_name = "ARGS"
    )]
    constructor_args: Vec<String>,

    /// The path to a file containing the constructor arguments.
    #[clap(
        long,
        help = "The path to a file containing the constructor arguments.",
        value_hint = ValueHint::FilePath,
        name = "constructor_args_path",
        conflicts_with = "constructor_args",
        value_name = "FILE"
    )]
    constructor_args_path: Option<PathBuf>,

    /// The factory dependencies in the form `<path>:<contractname>`.
    #[clap(
        long,
        num_args(1..),
        help_heading = "ZkSync Features",
        help = "The factory dependencies in the form `<path>:<contractname>`.",
        value_name = "FACTORY-DEPS"
    )]
    factory_deps: Option<Vec<ContractInfo>>,

    /// Core build arguments.
    #[clap(flatten)]
    opts: CoreBuildArgs,

    /// Transaction options, such as gas price and gas limit.
    #[clap(flatten)]
    tx: TransactionOpts,

    /// Ethereum-specific options, such as the network and wallet.
    #[clap(flatten)]
    eth: EthereumOpts,
}

impl ZkCreateArgs {
    /// Executes the command to create a contract.
    ///
    /// # Procedure
    /// 1. Retrieves private key, RPC URL, and chain information from the current instance.
    /// 2. It then sets up the project and artifact paths.
    /// 3. Retrieves the bytecode of the contract.
    /// 4. If factory dependencies are present, they are processed.
    /// 5. A signer is created using the private key and chain.
    /// 6. The ABI of the contract is obtained.
    /// 7. The constructor arguments are encoded.
    /// 8. A wallet is set up using the signer and the RPC URL.
    /// 9. The contract deployment is started.
    /// 10. If deployment is successful, the contract address, transaction hash, gas used, gas
    ///     price, and block number are printed to the console.
    pub async fn run(self) -> eyre::Result<()> {
        let private_key = get_private_key(&self.eth.wallet.raw.private_key)?;
        let rpc_url = get_rpc_url(&self.eth.rpc.url)?;
        let config = Config::from(&self.eth);
        let chain = get_chain(config.chain_id)?;
        let mut project = self.opts.project()?;
        project.paths.artifacts = project.paths.root.join("zkout");

        let bytecode = match Self::get_bytecode_from_contract(&project, &self.contract) {
            Ok(bytecode) => bytecode,
            Err(e) => {
                eyre::bail!("Error getting bytecode from contract: {}", e);
            }
        };

        //check for additional factory deps
        let factory_dependencies = self
            .factory_deps
            .as_ref()
            .map(|fdep_contract_info| self.get_factory_dependencies(&project, fdep_contract_info));

        // get abi
        let abi = match Self::get_abi_from_contract(&project, &self.contract) {
            Ok(abi) => abi,
            Err(e) => {
                eyre::bail!("Error gettting ABI from contract: {}", e);
            }
        };

        let contract = match serde_json::from_value(abi) {
            Ok(contract) => contract,
            Err(e) => {
                eyre::bail!("Error converting json abi to Contract ABI: {}", e);
            }
        };

        let constructor_args = self.get_constructor_args(&contract);

        let provider = Provider::try_from(rpc_url)?;
        let wallet = LocalWallet::from_str(&format!("{private_key:?}"))?.with_chain_id(chain);
        let zk_wallet = ZKSWallet::new(wallet, None, Some(provider), None)?;

        let rcpt = zk_wallet
            .deploy(contract, bytecode.to_vec(), constructor_args, factory_dependencies)
            .await?;

        let deployed_address = rcpt.contract_address.expect("Error retrieving deployed address");
        let gas_used = rcpt.gas_used.expect("Error retrieving gas used");
        let gas_price = rcpt.effective_gas_price.expect("Error retrieving gas price");
        let block_number = rcpt.block_number.expect("Error retrieving block number");

        println!("+-------------------------------------------------+");
        println!("Contract successfully deployed to address: {:#?}", deployed_address);
        println!("Transaction Hash: {:#?}", rcpt.transaction_hash);
        println!("Gas used: {:#?}", gas_used);
        println!("Effective gas price: {:#?}", gas_price);
        println!("Block Number: {:#?}", block_number);
        println!("+-------------------------------------------------+");

        Ok(())
    }

    /// This function retrieves the constructor arguments for the contract.
    ///
    /// # Returns
    /// A vector of `String` which represents the constructor arguments.
    /// An empty vector if there are no constructor arguments.
    fn get_constructor_args(&self, abi: &Abi) -> Vec<String> {
        if abi.constructor.is_some() {
            if let Some(ref constructor_args_path) = self.constructor_args_path {
                read_constructor_args_file(constructor_args_path.to_path_buf()).unwrap()
            } else {
                self.constructor_args.clone()
            }
        } else {
            vec![]
        }
    }

    /// This function retrieves the ABI from the contract.
    ///
    /// # Errors
    /// If there is an error in retrieving or parsing the ABI, it returns a serde_json::Error.
    ///
    /// # Returns
    /// A `Result` which is:
    /// - Ok: Contains the ABI as a serde_json::Value.
    /// - Err: Contains a serde_json::Error.
    fn get_abi_from_contract(
        project: &Project,
        contract_info: &ContractInfo,
    ) -> eyre::Result<Value> {
        let output_path = Self::get_path_for_contract_output(project, contract_info);
        let contract_output = Self::get_contract_output(output_path)?;
        serde_json::from_value(
            contract_output[contract_info.path.as_ref().unwrap()][&contract_info.name]["abi"]
                .clone(),
        )
        .wrap_err(format!(
            "Failed to find ABI for {} - is it the right contract name?",
            contract_info.name
        ))
    }

    /// This function retrieves the bytecode from the contract.
    ///
    /// # Procedure
    /// 1. Retrieves the contract info, checks if the contract's bytecode exists.
    /// 2. If the bytecode exists, it is decoded from hexadecimal representation into bytes.
    ///
    /// # Errors
    /// If there is an error in retrieving or decoding the bytecode, it returns an Error.
    ///
    /// # Returns
    /// A `Result` which is:
    /// - Ok: Contains the bytecode as a Bytes.
    /// - Err: Contains an error message indicating a problem with retrieving or decoding the
    ///   bytecode.
    fn get_bytecode_from_contract(
        project: &Project,
        contract_info: &ContractInfo,
    ) -> eyre::Result<Bytes> {
        let output_path = Self::get_path_for_contract_output(project, contract_info);
        let contract_output = Self::get_contract_output(output_path)?;
        let contract_file_codes = &contract_output[contract_info.path.as_ref().unwrap()];
        serde_json::from_value(
            contract_file_codes[&contract_info.name]["evm"]["bytecode"]["object"].clone(),
        )
        .wrap_err(format!(
            "Failed to find evm bytecode for {} - is this the correct contract name?",
            contract_info.name
        ))
    }

    /// This function retrieves the contract output.
    ///
    /// # Arguments
    ///
    /// * `output_path` - A `PathBuf` that represents the path to the contract output file.
    ///
    /// # Procedure
    ///
    /// 1. Reads the contract output file into a string.
    /// 2. Converts the string into a `serde_json::Value`.
    /// 3. Returns the "contracts" field from the JSON value.
    ///
    /// # Returns
    ///
    /// A `serde_json::Value` that represents the contract output.
    fn get_contract_output(output_path: PathBuf) -> eyre::Result<Value> {
        let data = fs::read_to_string(&output_path).wrap_err(format!(
            "Unable to read contract output file at {} - did you run zk-build",
            output_path.display()
        ))?;
        let res: serde_json::Value = serde_json::from_str(&data)
            .wrap_err(format!("Unable to parse JSON contract from {}", output_path.display()))?;
        Ok(res["contracts"].clone())
    }

    /// This function retrieves the path for the contract output.
    ///
    /// # Arguments
    ///
    /// * `project` - A `Project` instance that represents the current Solidity project.
    /// * `contract_info` - A `ContractInfo` instance that contains information about the contract.
    ///
    /// # Procedure
    ///
    /// 1. Retrieves the contract file path from `contract_info`.
    /// 2. Retrieves the contract file name from the file path.
    /// 3. Joins the artifacts path of the project with the contract file name.
    /// 4. Joins the resulting path with "artifacts.json" to create the path to the contract output.
    ///
    /// # Returns
    ///
    /// A `PathBuf` that represents the path to the contract output.
    fn get_path_for_contract_output(project: &Project, contract_info: &ContractInfo) -> PathBuf {
        let filepath = contract_info.path.clone().unwrap();
        let filename = filepath.split('/').last().unwrap();
        project.paths.artifacts.join(filename).join("artifacts.json")
    }

    /// This function retrieves the factory dependencies.
    ///
    /// # Arguments
    ///
    /// * `project` - A `Project` instance that represents the current Solidity project.
    /// * `factory_dep_vector` - A vector that contains the bytecode of each factory dependency
    ///   contract.
    /// * `fdep_contract_info` - A vector of `ContractInfo` instances that contain information about
    ///   each factory dependency contract.
    ///
    /// # Procedure
    ///
    /// 1. Iterates over each factory dependency contract in `fdep_contract_info`.
    /// 2. For each contract, retrieves its bytecode and appends it to `factory_dep_vector`.
    ///
    /// # Returns
    ///
    /// A vector of vectors of bytes that represents the bytecode of each factory dependency
    /// contract.
    fn get_factory_dependencies(
        &self,
        project: &Project,
        fdep_contract_info: &[ContractInfo],
    ) -> Vec<Vec<u8>> {
        let mut factory_deps = Vec::new();
        for dep in fdep_contract_info.iter() {
            let dep_bytecode = Self::get_bytecode_from_contract(project, dep).unwrap();
            factory_deps.push(dep_bytecode.to_vec());
        }
        factory_deps
    }
}
