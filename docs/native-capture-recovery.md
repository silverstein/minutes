# Recover a long native call recording

Minutes supervises native call capture with duration and disk limits. If neither source successfully writes audio for 30 seconds, it warns. After two minutes without verified progress, it stops through normal finalization only when fresh independent checks confirm that both stem files stopped growing. It preserves captured audio for processing or recovery. Silent PCM buffers count as progress. Missing helper health updates are reported separately from stalled audio writes. Growing stem files keep capture running when telemetry fails. Failed file checks remain unknown and do not trigger this watchdog. The monitor does not apply an acoustic silence policy to native capture.

For RF64/BW64 stems or RIFF stems that exceed 4 GB, recover each original stem into a new directory:

```sh
minutes recover-stem /path/to/call.system.wav --output-dir /path/to/system-recovery
minutes recover-stem /path/to/call.voice.wav --output-dir /path/to/voice-recovery
```

Each command preserves its original. It streams PCM into one-hour WAV parts without resampling, mixing, truncating or rewriting payload bytes. The parts keep the source sample rate and channels. `recovery.json` records every part's frame offset, frame count and PCM SHA256. Stdout contains the same JSON receipt.

Process a recovered part with the normal pipeline:

```sh
minutes process /path/to/system-recovery/part-0001.wav
```

The processing budget remains four hours per job. Use `--chunk-secs` to choose a shorter or longer part, from 1 to 14400 seconds, subject to RIFF's byte limit. Processing parts individually does not combine both sources or merge their transcripts. Keep both recovery receipts to align source parts by frame offset and sample rate. Automatic paired-source multipart processing is a follow-up.

Recovery requires enough disk space for the PCM copy plus a 1 GiB reserve. It refuses existing output directories, locked or changing inputs, malformed ds64 sizes, partial sample frames, more than 1024 parts, and work exceeding 30 minutes. RF64/BW64 inputs must have finalized, valid ds64 sizes. Oversized RIFF recovery can use a sane PCM format and terminal payload even if its size fields wrapped or were never finalized. Recovery does not guess a damaged PCM format.

On failure, the original and any complete published parts remain. An incomplete part is removed, and no completion receipt is written. Retry with another new directory after addressing the reported cause. Never delete the originals until you have verified the recovered audio and transcripts.

The RF64 and BW64 readers follow the size rules in [EBU Tech 3306](https://tech.ebu.ch/publications/tech3306) and [ITU-R BS.2088](https://www.itu.int/rec/R-REC-BS.2088).
