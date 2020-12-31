use crate::inventory::{Group, Inventory, Target};
use crate::transport::Transport;

use log::*;
use serde::{Deserialize, Serialize};
use ssh2::Session;
use std::convert::TryInto;
use std::io::prelude::*;
use std::io::BufReader;
use std::net::TcpStream;
use std::path::Path;

use zap_parser::plan::ExecutableTask;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Ssh {}

impl Default for Ssh {
    fn default() -> Self {
        Self {}
    }
}

impl Transport for Ssh {
    fn run_group(&self, command: &ExecutableTask, group: &Group, inventory: &Inventory) -> i32 {
        let mut status = 1;
        for target_name in group.targets.iter() {
            // XXX: This is inefficient
            for target in inventory.targets.iter() {
                if &target.name == target_name {
                    println!("Running on `{}`", target.name);
                    status = self.run(command, &target);
                }
            }
        }
        status
    }

    fn run(&self, command: &ExecutableTask, target: &Target) -> i32 {
        // Connect to the local SSH server
        let tcp = TcpStream::connect(format!("{}:22", target.uri)).unwrap();
        let mut sess = Session::new().unwrap();
        sess.set_tcp_stream(tcp);
        sess.handshake().unwrap();

        let mut authenticated = false;

        if let Some(config) = &target.config {
            if let Some(sshconfig) = &config.ssh {
                // requires PasswordAuthentication yes
                sess.userauth_password(&sshconfig.user, &sshconfig.password)
                    .unwrap();
                authenticated = true;
            }
        }
        if !authenticated {
            sess.userauth_agent(&std::env::var("USER").unwrap())
                .unwrap();
        }

        let remote_script = "._zap_command";

        if let Some(script) = command.task.script.as_bytes(Some(&command.parameters)) {
            let mut remote_file = sess
                .scp_send(
                    Path::new(remote_script),
                    0o700,
                    script
                        .len()
                        .try_into()
                        .expect("Overflow converting the size of the generated file, yikes!"),
                    None,
                )
                .unwrap();
            remote_file.write(&script).unwrap();
            // Close the channel and wait for the whole content to be tranferred
            remote_file.send_eof().unwrap();
            remote_file.wait_eof().unwrap();
            remote_file.close().unwrap();
            remote_file.wait_close().unwrap();

            let mut channel = sess.channel_session().unwrap();
            let stderr = channel.stderr();

            channel.exec(&format!("./{}", remote_script)).unwrap();

            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                println!("err: {}", line.unwrap());
            }

            let mut s = String::new();
            channel.read_to_string(&mut s).unwrap();
            print!("{}", s);
            channel.wait_close().expect("Failed to close the channel");
            let exit = channel.exit_status().unwrap();

            /*
             * This seems a little dumb and hacky, but we need to clean up the file
             * somehow and I'm not seeing anything that would allow me to just reach
             * out and remove a file
             */
            let mut channel = sess.channel_session().unwrap();
            channel.exec(&format!("rm -f {}", remote_script)).unwrap();
            return exit;
        } else {
            error!("No script available to run for task!");
            return -1;
        }
    }
}
