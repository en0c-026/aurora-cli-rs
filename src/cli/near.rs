use crate::{client::AuroraClient, config::Config, utils};
use aurora_engine_types::{
    parameters::{CrossContractCallArgs, PromiseArgs, PromiseCreateArgs},
    types::{Address, NearGas, Wei, Yocto},
    U256,
};
use borsh::BorshSerialize;
use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    Read {
        #[clap(subcommand)]
        subcommand: ReadCommand,
    },
    Write {
        #[clap(subcommand)]
        subcommand: WriteCommand,
    },
}

#[derive(Subcommand)]
pub enum ReadCommand {
    GetReceiptResult { receipt_id_b58: String },
}

#[derive(Subcommand)]
pub enum WriteCommand {
    EngineXcc {
        #[clap(short, long)]
        target_near_account: String,
        #[clap(short, long)]
        method_name: String,
        #[clap(short, long)]
        json_args: Option<String>,
        #[clap(long)]
        json_args_stdin: Option<bool>,
        #[clap(short, long)]
        deposit_yocto: Option<String>,
        #[clap(short, long)]
        attached_gas: Option<String>,
    },
}

pub async fn execute_command<T: AsRef<str>>(
    command: Command,
    client: &AuroraClient<T>,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    match command {
        Command::Read { subcommand } => match subcommand {
            ReadCommand::GetReceiptResult { receipt_id_b58 } => {
                let tx_hash = bs58::decode(receipt_id_b58.as_str()).into_vec().unwrap();
                let outcome = client
                    .get_near_receipt_outcome(tx_hash.as_slice().try_into().unwrap())
                    .await?;
                println!("{:?}", outcome);
            }
        },
        Command::Write { subcommand } => match subcommand {
            WriteCommand::EngineXcc {
                target_near_account,
                method_name,
                json_args,
                json_args_stdin,
                deposit_yocto,
                attached_gas,
            } => {
                let source_private_key_hex = config.get_evm_secret_key();
                let sk_bytes = utils::hex_to_arr32(source_private_key_hex)?;
                let sk = secp256k1::SecretKey::parse(&sk_bytes).unwrap();
                let near_args = match json_args {
                    Some(args) => args.into_bytes(),
                    None => match json_args_stdin {
                        Some(true) => {
                            let mut buf = String::new();
                            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf).unwrap();
                            buf.into_bytes()
                        }
                        None | Some(false) => Vec::new(),
                    },
                };
                let attached_balance = match deposit_yocto {
                    Some(x) => Yocto::new(x.parse().unwrap()),
                    None => Yocto::new(0),
                };
                // TODO: there is an issue with the NEAR nonce tracking if I do two calls in a row
                /*if attached_balance.as_u128() > 0 {
                    // If we want to spend NEAR then we need to approve the precompile to spend our wNEAR.
                    const APPROVE_SELECTOR: &[u8] = &[0x09u8, 0x5e, 0xa7, 0xb3];
                    let input = [APPROVE_SELECTOR, &ethabi::encode(&[
                        ethabi::Token::Address(aurora_engine_precompiles::xcc::cross_contract_call::ADDRESS.raw()),
                        ethabi::Token::Uint(U256::from(u128::MAX)),
                    ])].concat();
                    let result = send_as_near_transaction(
                        &client,
                        &sk,
                        Address::decode("34aadb3d3f359c7bfefa87f7a0ed4dbe5ba17d78").ok(),
                        Wei::zero(),
                        input,
                    )
                    .await?;
                    println!("APPROVE: {:?}\n\n", result);
                }*/
                let attached_gas = match attached_gas {
                    Some(gas) => NearGas::new(gas.parse().unwrap()),
                    None => NearGas::new(30_000_000_000_000),
                };
                let promise = PromiseArgs::Create(PromiseCreateArgs {
                    target_account_id: target_near_account.parse().unwrap(),
                    method: method_name,
                    args: near_args,
                    attached_balance,
                    attached_gas,
                });
                let precompile_args = CrossContractCallArgs::Eager(promise);
                let result = send_as_near_transaction(
                    client,
                    &sk,
                    Some(aurora_engine_precompiles::xcc::cross_contract_call::ADDRESS),
                    Wei::zero(),
                    precompile_args.try_to_vec().unwrap(),
                )
                .await?;
                println!("{:?}", result);
            }
        },
    };
    Ok(())
}

async fn send_as_near_transaction<T: AsRef<str>>(
    client: &AuroraClient<T>,
    sk: &secp256k1::SecretKey,
    to: Option<Address>,
    amount: Wei,
    input: Vec<u8>,
) -> Result<near_primitives::views::FinalExecutionOutcomeView, Box<dyn std::error::Error>> {
    let sender_address = utils::address_from_secret_key(sk);
    let nonce = {
        let result = client
            .near_view_call("get_nonce".into(), sender_address.as_bytes().to_vec())
            .await?;
        U256::from_big_endian(&result.result)
    };
    let tx = aurora_engine_transactions::legacy::TransactionLegacy {
        nonce,
        gas_price: U256::zero(),
        gas_limit: U256::from(u64::MAX),
        to,
        value: amount,
        data: input,
    };
    let chain_id = {
        let result = client
            .near_view_call("get_chain_id".into(), sender_address.as_bytes().to_vec())
            .await?;
        U256::from_big_endian(&result.result).low_u64()
    };
    let signed_tx = aurora_engine_transactions::EthTransactionKind::Legacy(
        utils::sign_transaction(tx, chain_id, sk),
    );
    let result = client
        .near_contract_call("submit".into(), (&signed_tx).into())
        .await?;
    Ok(result)
}
