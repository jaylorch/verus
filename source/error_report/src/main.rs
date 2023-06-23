use chrono::{prelude::*, DateTime};
use std::{
    env,
    fs::{self, File},
    io::prelude::*,
    io::{BufRead, BufReader},
    path::Path,
    process::{Command, Stdio},
    str,
};
use toml::{map::Map, value::Value};
use zip::write::FileOptions;

fn main() {
    let mut file_path = String::new();
    #[allow(unused_assignments)]
    let mut our_args = Vec::new();

    let args: Vec<String> = env::args().collect();
    if args.len() > 1 {
        for argument in &args {
            if argument.ends_with(".rs") {
                file_path = argument.clone();
            }
        }
        our_args = args[2..].to_vec();
    } else {
        println!("Usage: error_report <file_name>");
        return;
    }

    let program_dir = args[1].clone();

    let z3_path = Path::new(&program_dir).join("z3");
    let verus_path = Path::new(&program_dir).join("verus");

    let z3_version_output =
        Command::new(z3_path).arg("--version").output().expect("failed to execute process");
    let verus_version_output =
        Command::new(&verus_path).arg("--version").output().expect("failed to execute process");

    let child = Command::new(verus_path)
        .stdin(Stdio::null())
        .args(our_args)
        .arg("--emit=dep-info")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to execute process");
    let verus_output: std::process::Output =
        child.wait_with_output().expect("Failed to read stdout");

    // The following method calls do the actual work of writing a toml file
    // with relevant information and saving the toml file and relevant files
    // to a zip file
    toml_setup_and_write(args, z3_version_output, verus_version_output, verus_output);
    let (d_file_name, zip_file_name) = zip_setup(file_path);
    println!("Stored error report to {}\n", zip_file_name);

    fs::remove_file("error_report.toml").expect("failed to delete toml file\n");
    fs::remove_file(d_file_name).expect("failed to delete .d file\n");
}

/// Transforms data from the input file into the proper data structure for
/// toml creation, and then calls a function to write the toml
///
/// Parameters: args: The command line arguments given to call the input file
///             z3_version_output: Information regarding the user's current z3 version
///             verus_version_output: Information regarding the user's current verus version
///             verus_output: The resulting output from the input file
fn toml_setup_and_write(
    args: Vec<String>,
    z3_version_output: std::process::Output,
    verus_version_output: std::process::Output,
    verus_output: std::process::Output,
) {
    let z3_version = str::from_utf8(&z3_version_output.stdout)
        .expect("got non UTF-8 data from z3 version output")
        .to_string();
    let verus_version = str::from_utf8(&verus_version_output.stdout)
        .expect("got non UTF-8 data from verus version output")
        .to_string();
    let stdout =
        str::from_utf8(&verus_output.stdout).expect("got non UTF-8 data from stdout").to_string();
    let stderr =
        str::from_utf8(&verus_output.stderr).expect("got non UTF-8 data from stderr").to_string();

    let toml_string =
        toml::to_string(&create_toml(args, z3_version, verus_version, stdout, stderr))
            .expect("Could not encode TOML value");
    fs::write("error_report.toml", toml_string).expect("Could not write to file!");
}

/// Creates a toml file and writes relevant information to this file, including
/// the command-line arguments, versions, and output.
///
/// Parameters: args: The command line arguments given to call the input file
///             z3_version: Information regarding the user's current z3 version
///             verus_version: Information regarding the user's current verus version
///             stdout: The resulting output from the input file to stdout
///             stderr: The resulting output from the input file to stderr
///  
/// Returns:    A Table data structure used to write a toml file
fn create_toml(
    args: Vec<String>,
    z3_version: String,
    verus_version: String,
    stdout: String,
    stderr: String,
) -> Value {
    let mut command_line_arguments = Map::new();
    command_line_arguments.insert("args".to_string(), Value::String(args.join(" ")));

    let mut versions = Map::new();
    versions.insert("z3-version".to_string(), Value::String(z3_version));
    versions.insert("verus-version".to_string(), Value::String(verus_version));
    let mut output = Map::new();
    output.insert("stdout".to_string(), Value::String(stdout));
    output.insert("stderr".to_string(), Value::String(stderr));
    let mut map = Map::new();
    map.insert(
        "title".to_string(),
        Value::String("Error report file - details and dependencies".to_string()),
    );
    map.insert("report-schema-version".into(), Value::String("1".to_string()));
    map.insert("command-line-arguments".into(), Value::Table(command_line_arguments));
    map.insert("versions".into(), Value::Table(versions));
    map.insert("verus-output".into(), Value::Table(output));
    Value::Table(map)
}

/// Uses the user input file to find the .d file, parse the dependencies,
/// and write each dependency to the zip file.
///
/// Parameters: file_path: a String representation of the path to the input file
///
/// Returns:    the names of the .d file and zip file for book-keeping purposes
pub fn zip_setup(file_path: String) -> (String, String) {
    let file_name_path = Path::new(&file_path);
    let temp_file_name =
        &file_name_path.with_extension(".d").file_name().unwrap().to_string_lossy().to_string();
    let mut d_file_name = String::new();
    d_file_name.push_str(&temp_file_name.to_string()[..]);
    d_file_name = d_file_name[..d_file_name.len() - 2].to_string();
    d_file_name.push('d');
    let mut deps = d_to_vec(d_file_name.to_string());
    deps.push("error_report.toml".to_string());
    let zip_file_name = write_zip_archive(deps);
    (d_file_name, zip_file_name)
}

/// Turns the .d file that lists each of the input files' dependencies
/// and turns them into a vector of Strings for easier data manipulation
///
/// Parameters: file_name: The name of the previously generated .d file
///
/// Returns:    a vector containing each dependency of the input file as an
///             individual string
fn d_to_vec(file_name: String) -> Vec<String> {
    let file = File::open(file_name).expect("Couldn't open file!");
    let mut reader = BufReader::new(file);
    let mut dependencies = String::new();
    reader.read_line(&mut dependencies).expect("Could not read the first line");
    dependencies.split_whitespace().skip(1).map(|x| x.to_string()).collect()
}

/// Creates a zip file from a given list of files to compress
///
/// Parameters: deps: A vector of strings representing files to be compressed
///                    (in this context, each file is a dependency of the input)
///
/// Returns:    The name of the created zip file
fn write_zip_archive(deps: Vec<String>) -> String {
    let local: DateTime<Local> = Local::now();
    let date = local.to_string();
    let mut zip_file_name = date[0..19].to_string();
    zip_file_name.push_str(".zip");
    zip_file_name = zip_file_name.replace(" ", "-");
    zip_file_name = zip_file_name.replace(":", "-");

    let path = std::path::Path::new(&zip_file_name);
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default()
        .compression_method(zip::CompressionMethod::Bzip2)
        .unix_permissions(0o644);
    for file in deps {
        let path = file;
        let binding = fs::read_to_string(&path).expect("Could not read file");
        let content = binding.as_bytes();

        zip.start_file(path, options).expect("Could not start file");
        zip.write_all(content).expect("Could not write file contents to zip");
    }
    zip.finish().expect("Could not finish up zip file");
    zip_file_name
}