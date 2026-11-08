use lockbox_key_import::imported_key_dealer::{deal_imported_key, parse_secret_key_hex, write_dealer_output, ImportRecipient};
use serde_json::json;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use zeroize::Zeroizing;

const USAGE: &str = "usage: lockbox-key-import-dealer --key-ref KR --expected-address 0x.. --threshold 2 \
--recipient s1=signer1-certificate.pem --recipient s2=.. --recipient s3=.. --output-directory DIR  (secret key in hex on stdin)";
const SECRET_INPUT_CAPACITY_BYTES: usize = 4096;

struct DealerArguments {
    key_ref: String,
    expected_venue_address: String,
    threshold: u16,
    recipient_certificate_files: Vec<(String, PathBuf)>,
    output_directory: PathBuf,
}

fn parse_arguments() -> Result<DealerArguments, String> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut key_ref = None;
    let mut expected_venue_address = None;
    let mut threshold = None;
    let mut recipient_certificate_files = Vec::new();
    let mut output_directory = None;
    let mut position = 0;
    while position < arguments.len() {
        let value = arguments.get(position + 1).cloned().ok_or(USAGE)?;
        match arguments[position].as_str() {
            "--key-ref" => key_ref = Some(value),
            "--expected-address" => expected_venue_address = Some(value),
            "--threshold" => threshold = Some(value.parse::<u16>().map_err(|_| "threshold is not a number")?),
            "--recipient" => {
                let (signer_id, certificate_file) = value.split_once('=').ok_or(USAGE)?;
                recipient_certificate_files.push((signer_id.to_string(), PathBuf::from(certificate_file)));
            }
            "--output-directory" => output_directory = Some(PathBuf::from(value)),
            _ => return Err(USAGE.to_string()),
        }
        position += 2;
    }
    let expected_venue_address = expected_venue_address.ok_or(USAGE)?;
    if !(expected_venue_address.len() == 42 && expected_venue_address.starts_with("0x") && hex::decode(&expected_venue_address[2..]).is_ok()) {
        return Err("expected address must be a 0x Ethereum address".to_string());
    }
    recipient_certificate_files.sort();
    Ok(DealerArguments {
        key_ref: key_ref.ok_or(USAGE)?,
        expected_venue_address,
        threshold: threshold.ok_or(USAGE)?,
        recipient_certificate_files,
        output_directory: output_directory.ok_or(USAGE)?,
    })
}

fn read_secret_from_standard_input() -> Result<Zeroizing<String>, String> {
    if std::io::stdin().is_terminal() {
        return rpassword::prompt_password("secret key (hex, not shown): ").map(Zeroizing::new).map_err(|error| format!("read secret: {error}"));
    }
    let mut secret_text = Zeroizing::new(String::with_capacity(SECRET_INPUT_CAPACITY_BYTES));
    std::io::stdin()
        .take(SECRET_INPUT_CAPACITY_BYTES as u64 - 1)
        .read_to_string(&mut secret_text)
        .map_err(|error| format!("read secret: {error}"))?;
    Ok(secret_text)
}

fn run() -> Result<String, String> {
    let arguments = parse_arguments()?;
    let mut recipients = Vec::new();
    for (signer_id, certificate_file) in &arguments.recipient_certificate_files {
        let certificate_pem =
            std::fs::read_to_string(certificate_file).map_err(|error| format!("read {}: {error}", certificate_file.display()))?;
        recipients.push(ImportRecipient { signer_id: signer_id.clone(), certificate_pem });
    }
    if arguments.output_directory.exists() && std::fs::read_dir(&arguments.output_directory).map_err(|error| error.to_string())?.next().is_some() {
        return Err(format!("output directory {} is not empty", arguments.output_directory.display()));
    }
    let secret_key = parse_secret_key_hex(read_secret_from_standard_input()?)?;
    let dealer_output = deal_imported_key(secret_key, &arguments.expected_venue_address, &arguments.key_ref, &recipients, arguments.threshold)?;
    write_dealer_output(&arguments.output_directory, &dealer_output)?;
    Ok(json!({
        "key_ref": dealer_output.public_data.key_ref,
        "venue_address": dealer_output.public_data.venue_address,
        "committee": dealer_output.public_data.committee,
        "threshold": dealer_output.public_data.threshold,
        "output_directory": arguments.output_directory.display().to_string(),
    })
    .to_string())
}

fn main() {
    match run() {
        Ok(summary) => println!("{summary}"),
        Err(reason) => {
            eprintln!("refused: {reason}");
            std::process::exit(1);
        }
    }
}
