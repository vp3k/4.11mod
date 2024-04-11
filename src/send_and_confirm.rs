use std::{
    io::{stdout, Write},
    time::Duration,
};

use solana_client::{
    client_error::{ClientError, ClientErrorKind, Result as ClientResult},
    rpc_config::{RpcSendTransactionConfig, RpcSimulateTransactionConfig},
};
use solana_program::instruction::Instruction;
use solana_sdk::{
    commitment_config::CommitmentLevel,
    compute_budget::ComputeBudgetInstruction,
    signature::{Signature, Signer},
    transaction::Transaction,
};
use solana_transaction_status::{TransactionConfirmationStatus, UiTransactionEncoding};

use crate::Miner;

const RPC_RETRIES: usize = 0;
const SIMULATION_RETRIES: usize = 4;
const GATEWAY_RETRIES: usize = 4;
const CONFIRM_RETRIES: usize = 4;

const CONFIRM_DELAY: u64 = 5000;
const GATEWAY_DELAY: u64 = 2000;

impl Miner {
    pub async fn send_and_confirm_batch(
        &self,
        txs_ixs: Vec<Vec<Instruction>>,
        dynamic_cus: bool,
        skip_confirm: bool,
    ) -> ClientResult<Vec<Signature>> {
        let mut stdout = stdout();
        let signer = self.signer();
        let client = self.rpc_client.clone();
        let mut signatures = Vec::new();

        for ixs in txs_ixs.iter() {
            let balance = client.get_balance(&signer.pubkey()).await?;
            if balance <= 0 {
                return Err(ClientError {
                    request: None,
                    kind: ClientErrorKind::Custom("Insufficient SOL balance".into()),
                });
            }

            let (mut hash, mut slot) = client
                .get_latest_blockhash_with_commitment(self.rpc_client.commitment())
                .await?;

            let mut tx = Transaction::new_with_payer(ixs, Some(&signer.pubkey()));

            if dynamic_cus {
                let mut sim_attempts = 0;
                'simulate: loop {
                    let sim_res = client
                        .simulate_transaction_with_config(
                            &tx,
                            RpcSimulateTransactionConfig {
                                sig_verify: false,
                                replace_recent_blockhash: true,
                                commitment: Some(self.rpc_client.commitment()),
                                encoding: Some(UiTransactionEncoding::Base64),
                                accounts: None,
                                min_context_slot: None,
                                inner_instructions: false,
                            },
                        )
                        .await;
                    match sim_res {
                        Ok(sim_res) => {
                            if let Some(err) = sim_res.value.err {
                                println!("Simulation error: {:?}", err);
                                sim_attempts += 1;
                                if sim_attempts > SIMULATION_RETRIES {
                                    return Err(ClientError {
                                        request: None,
                                        kind: ClientErrorKind::Custom("Simulation failed".into()),
                                    });
                                }
                            } else if let Some(units_consumed) = sim_res.value.units_consumed {
                                println!("Dynamic CUs: {:?}", units_consumed);
                                let cu_budget_ix = ComputeBudgetInstruction::set_compute_unit_limit(
                                    units_consumed as u32 + 1000,
                                );
                                let cu_price_ix =
                                    ComputeBudgetInstruction::set_compute_unit_price(self.priority_fee);
                                let mut final_ixs = vec![cu_budget_ix, cu_price_ix];
                                final_ixs.extend_from_slice(ixs);
                                tx = Transaction::new_with_payer(&final_ixs, Some(&signer.pubkey()));
                                break 'simulate;
                            }
                        }
                        Err(err) => {
                            println!("Simulation error: {:?}", err);
                            sim_attempts += 1;
                            if sim_attempts > SIMULATION_RETRIES {
                                return Err(ClientError {
                                    request: None,
                                    kind: ClientErrorKind::Custom("Simulation failed".into()),
                                });
                            }
                        }
                    }
                }
            }

            tx.sign(&[&signer], hash);
            let send_cfg = RpcSendTransactionConfig {
                skip_preflight: true,
                preflight_commitment: Some(CommitmentLevel::Finalized),
                encoding: Some(UiTransactionEncoding::Base64),
                max_retries: Some(RPC_RETRIES),
                min_context_slot: Some(slot),
            };

            let mut attempts = 0;
            loop {
                match client.send_transaction_with_config(&tx, send_cfg.clone()).await {
                    Ok(sig) => {
                        signatures.push(sig);
                        if skip_confirm {
                            break;
                        }
                        // Confirm transaction logic here
                        // This is simplified; you'll need to implement actual confirmation logic
                        println!("Transaction submitted with signature: {:?}", sig);
                        break;
                    }
                    Err(err) => {
                        println!("Error submitting transaction: {:?}", err);
                        attempts += 1;
                        if attempts > GATEWAY_RETRIES {
                            return Err(ClientError {
                                request: None,
                                kind: ClientErrorKind::Custom("Max retries exceeded".into()),
                            });
                        }
                        std::thread::sleep(Duration::from_millis(GATEWAY_DELAY));
                    }
                }
            }
        }

        Ok(signatures)
    }
}
