use crate::{
    cmd::*,
    result::Result,
    traits::{TxnEnvelope, TxnFee, TxnSign, B64},
};

#[derive(Debug, StructOpt)]
/// Onboard a given encoded validator staking transactiom with this wallet.
/// transaction signed by the Helium staking server.
pub enum Cmd {
    Create(Create),
    Accept(Accept),
}

#[derive(Debug, StructOpt)]
/// Onboard a given encoded validator staking transactiom with this wallet.
/// transaction signed by the Helium staking server. The current (old) or new
/// owner are set to the public key of the given wallet if not specified.
pub struct Create {
    /// The validator to transfer the stake from
    #[structopt(long)]
    old_address: PublicKey,

    /// The validator to transfer the stake to
    #[structopt(long)]
    new_address: PublicKey,

    /// The new owner of the transfered validator and stake. If not present the
    /// the new owner is assumed to be the same as the current owner as defined
    /// on the blockchain.
    #[structopt(long)]
    new_owner: Option<PublicKey>,

    /// The current (old) owner of the transfered validator and stake. If not present the
    /// it is set to the public key of the given wallet.
    #[structopt(long)]
    old_owner: Option<PublicKey>,

    /// The amount of HNT to transfer from the new to the old owner as part of
    /// the stake transfer
    #[structopt(long, default_value = "0")]
    amount: Hnt,

    /// Whether to commit the transaction to the blockchain
    #[structopt(long)]
    commit: bool,
}

#[derive(Debug, StructOpt)]
/// Accept a given stake transfer transaction by signing it and committing to
/// the API if requested. The transaction is signed as either (or both) the new
/// owner or the old owner if the owner keys match the public key of the given
/// wallet.
pub struct Accept {
    /// Base64 encoded transaction to sign. If no transaction if given
    /// stdin is read for the transaction. Note that the stdin feature
    /// only works if the wallet password is set in the
    /// HELIUM_WALLET_PASSWORD environment variable
    #[structopt(name = "TRANSACTION")]
    txn: Option<Transaction>,

    /// Whether to commit the transaction to the blockchain
    #[structopt(long)]
    commit: bool,
}

impl Cmd {
    pub async fn run(&self, opts: Opts) -> Result {
        match self {
            Self::Accept(cmd) => cmd.run(opts).await,
            Self::Create(cmd) => cmd.run(opts).await,
        }
    }
}

impl Create {
    pub async fn run(&self, opts: Opts) -> Result {
        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        let client = helium_api::Client::new_with_base_url(api_url(wallet.public_key.network));

        let old_owner = self.old_owner.as_ref().unwrap_or(&wallet.public_key);
        let stake_amount = helium_api::validators::get(&client, &wallet.public_key.to_string())
            .await?
            .stake;

        let mut txn = BlockchainTxnTransferValidatorStakeV1 {
            old_address: self.old_address.to_vec(),
            new_address: self.new_address.to_vec(),
            old_owner: old_owner.to_vec(),
            new_owner: self
                .new_owner
                .as_ref()
                .map(|o| o.to_vec())
                .unwrap_or_else(Vec::new),
            fee: 0,
            stake_amount,
            payment_amount: u64::from(self.amount),
            old_owner_signature: vec![],
            new_owner_signature: vec![],
        };

        txn.fee = txn.txn_fee(&get_txn_fees(&client).await?)?;
        if old_owner == &wallet.public_key {
            txn.old_owner_signature = txn.sign(&keypair)?;
        }
        if let Some(owner) = &self.new_owner {
            if owner == &wallet.public_key {
                txn.new_owner_signature = txn.sign(&keypair)?;
            }
        }

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(Some(&envelope), &txn, &status, opts.format)
    }
}

impl Accept {
    pub async fn run(&self, opts: Opts) -> Result {
        let mut txn = BlockchainTxnTransferValidatorStakeV1::from_envelope(&read_txn(&self.txn)?)?;

        let password = get_password(false)?;
        let wallet = load_wallet(opts.files)?;
        let keypair = wallet.decrypt(password.as_bytes())?;

        if !txn.old_owner.is_empty() && PublicKey::from_bytes(&txn.old_owner)? == wallet.public_key
        {
            txn.old_owner_signature = txn.sign(&keypair)?;
        }
        if !txn.new_owner.is_empty() && PublicKey::from_bytes(&txn.new_owner)? == wallet.public_key
        {
            txn.new_owner_signature = txn.sign(&keypair)?;
        }

        let client = helium_api::Client::new_with_base_url(api_url(wallet.public_key.network));

        let envelope = txn.in_envelope();
        let status = maybe_submit_txn(self.commit, &client, &envelope).await?;
        print_txn(None, &txn, &status, opts.format)
    }
}

fn print_txn(
    envelope: Option<&BlockchainTxn>,
    txn: &BlockchainTxnTransferValidatorStakeV1,
    status: &Option<PendingTxnStatus>,
    format: OutputFormat,
) -> Result {
    let old_address = PublicKey::from_bytes(&txn.old_address)?.to_string();
    let new_address = PublicKey::from_bytes(&txn.new_address)?.to_string();
    let old_owner = PublicKey::from_bytes(&txn.old_owner)?.to_string();
    let new_owner = if txn.new_owner.is_empty() {
        "current".to_string()
    } else {
        PublicKey::from_bytes(&txn.new_owner)?.to_string()
    };
    match format {
        OutputFormat::Table => {
            ptable!(
                ["Key", "Value"],
                ["Old address", old_address],
                ["New address", new_address],
                ["Old owner", old_owner],
                ["New owner", new_owner],
                ["Fee", txn.fee],
                ["Amount", Hnt::from(txn.payment_amount)],
                ["Hash", status_str(status)]
            );
            print_footer(status)
        }
        OutputFormat::Json => {
            let mut table = json!({
                "old_address" : old_address,
                "new_address" : new_address,
                "old_owner" : old_owner,
                "new_owner" : new_owner,
                "fee": txn.fee,
                "amount": Hnt::from(txn.payment_amount),
                "hash": status_json(status)
            });
            if let Some(envelope) = envelope {
                table["txn"] = envelope.to_b64()?.into();
            };
            print_json(&table)
        }
    }
}
