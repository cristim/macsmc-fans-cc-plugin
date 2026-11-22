use std::{
    fs::{self, File},
    io,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
};

static SYSFS_ROOT: &str =
    "/sys/module/macsmc_hwmon/drivers/platform:macsmc-hwmon/macsmc-hwmon/hwmon";

pub(crate) async fn probe() -> io::Result<Vec<Fan>> {
    // Find the first hwmon device and use it.
    let root = find_attr_root().await?.unwrap();
    let mut read = tokio::fs::read_dir(&root).await?;
    let mut fans = Vec::new();

    while let Some(entry) = read.next_entry().await? {
        if let Ok(file_name) = entry.file_name().into_string() {
            if let Some(s) = file_name.strip_prefix("fan") {
                if let Some(n) = s.strip_suffix("_target") {
                    log::info!("discovered fan: {}", entry.path().display());
                    fans.push(Fan::new(&root, n.parse().unwrap())?);
                }
            }
        }
    }

    Ok(fans)
}

async fn find_attr_root() -> io::Result<Option<PathBuf>> {
    let mut read = tokio::fs::read_dir(SYSFS_ROOT).await?;

    while let Some(entry) = read.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            let path = entry.path();

            if path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("hwmon"))
                .unwrap_or(false)
            {
                return Ok(Some(path));
            }
        }
    }

    Ok(None)
}

/// Wrapper around the sysfs API.
pub(crate) struct Fan {
    id: u8,
    label: String,
    input: File,
    target: File,
    min_rpm: u32,
    max_rpm: u32,
}

impl Fan {
    fn new(root: &Path, id: u8) -> io::Result<Self> {
        let label = fs::read_to_string(root.join(format!("fan{id}_label")))?
            .trim()
            .trim()
            .to_owned();
        let min_rpm = fs::read_to_string(root.join(format!("fan{id}_min")))?
            .trim()
            .parse()
            .unwrap();
        let max_rpm = fs::read_to_string(root.join(format!("fan{id}_max")))?
            .trim()
            .parse()
            .unwrap();
        // With a minimum of 0 the kernel would take a release (target 0) as a manual stop.
        if min_rpm == 0 || min_rpm > max_rpm {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("fan{id} reports an invalid speed range {min_rpm}..{max_rpm} rpm"),
            ));
        }

        Ok(Self {
            id,
            label,
            input: File::open(root.join(format!("fan{id}_input")))?,
            target: File::options()
                .read(false)
                .write(true)
                .open(root.join(format!("fan{id}_target")))?,
            min_rpm,
            max_rpm,
        })
    }

    pub(crate) fn id(&self) -> u8 {
        self.id
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }

    pub(crate) fn max_rpm(&self) -> u32 {
        self.max_rpm
    }

    /// Get the current fan speed measured in RPM.
    ///
    /// This will fluctuate over time, since the fan cannot perfectly maintain
    /// an exact target RPM.
    pub(crate) fn get_current_rpm(&self) -> io::Result<u32> {
        let mut buf = [0; 8];
        self.input.read_at(&mut buf, 0)?;

        Ok(lexical::parse_partial(&buf).unwrap().0)
    }

    /// Set the desired RPM that this fan should spin at.
    pub(crate) fn set_target_rpm(&self, rpm: u32) -> io::Result<()> {
        self.target
            .write_at(lexical::to_string(rpm).as_bytes(), 0)?;

        Ok(())
    }

    pub(crate) fn min_duty(&self) -> u32 {
        min_duty(self.min_rpm, self.max_rpm)
    }

    pub(crate) fn duty_to_rpm(&self, duty: i32) -> u32 {
        duty_to_rpm(duty, self.min_rpm, self.max_rpm)
    }

    /// Return the fan to automatic control by the SMC firmware.
    ///
    /// The kernel driver treats a target of 0 as a request to leave manual mode.
    pub(crate) fn release_to_firmware(&self) -> io::Result<()> {
        self.set_target_rpm(0)
    }
}

/// Release every fan, attempting all of them even if one fails.
pub(crate) fn release_all(fans: &[Fan]) -> io::Result<()> {
    let mut result = Ok(());
    for fan in fans {
        if let Err(err) = fan.release_to_firmware() {
            log::error!("failed to release fan {} to firmware: {err}", fan.id());
            result = Err(err);
        }
    }
    result
}

/// Lowest duty whose target still reaches min_rpm; the kernel rejects non-zero targets below it.
fn min_duty(min_rpm: u32, max_rpm: u32) -> u32 {
    (min_rpm * 100).div_ceil(max_rpm)
}

/// Clamp so a low or negative duty never writes 0, which would hand the fan back to firmware.
fn duty_to_rpm(duty: i32, min_rpm: u32, max_rpm: u32) -> u32 {
    let duty = duty.clamp(min_duty(min_rpm, max_rpm) as i32, 100) as u32;
    max_rpm * duty / 100
}

#[cfg(test)]
mod tests {
    use super::*;

    // fan1_min/fan1_max from a 13-inch M1 MacBook Pro and a 14-inch MacBook Pro.
    const FANS: [(u32, u32); 2] = [(1199, 7199), (2317, 6550)];

    #[test]
    fn min_duty_is_the_lowest_duty_reaching_min_rpm() {
        assert_eq!(min_duty(1199, 7199), 17);
        assert_eq!(min_duty(2317, 6550), 36);
        for (min, max) in FANS {
            let lowest = min_duty(min, max);
            assert!(max * lowest / 100 >= min);
            assert!(max * (lowest - 1) / 100 < min);
        }
    }

    #[test]
    fn every_duty_maps_to_a_target_the_kernel_accepts() {
        for (min, max) in FANS {
            for duty in -50..=150 {
                let rpm = duty_to_rpm(duty, min, max);
                assert!((min..=max).contains(&rpm), "duty {duty} gave {rpm} rpm");
            }
        }
    }

    #[test]
    fn low_duty_clamps_to_min_duty_instead_of_releasing() {
        assert_eq!(duty_to_rpm(0, 1199, 7199), duty_to_rpm(17, 1199, 7199));
        assert_eq!(duty_to_rpm(-1, 1199, 7199), 1223);
        assert_eq!(duty_to_rpm(100, 1199, 7199), 7199);
    }
}
