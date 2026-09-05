"""The real app's existing minimal FFmpeg 8.1.2 recipe, in the shared builder."""

import hashlib
from pathlib import Path

CONFIGURE = "./configure --prefix=/workspace/apps/mister/target/ffmpeg-minimal/armv7/dist --cross-prefix=arm-linux-gnueabihf- --arch=arm --cpu=cortex-a9 --target-os=linux --enable-cross-compile --extra-cflags='-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard' --extra-cxxflags='-O3 -mcpu=cortex-a9 -mfpu=neon-vfpv3 -mfloat-abi=hard' --enable-static --disable-shared --enable-pic --disable-autodetect --disable-programs --disable-doc --disable-debug --enable-stripping --disable-everything --disable-avdevice --disable-avfilter --enable-swresample --enable-avcodec --enable-avformat --enable-avutil --disable-swscale --enable-decoder=h264 --enable-decoder=aac --enable-decoder=pcm_s16le --enable-parser=aac --enable-parser=h264 --enable-demuxer=mov --enable-protocol=file"


def prepare_ffmpeg(repository: Path, container: str, runner) -> None:
    work = repository / "apps/mister/target/ffmpeg-minimal/armv7"
    stamp = work / "dist/.magik2-recipe"
    fingerprint = hashlib.sha256(CONFIGURE.encode()).hexdigest()
    libraries = ("avcodec", "avformat", "avutil", "swresample")
    if (
        stamp.is_file()
        and stamp.read_text() == fingerprint
        and all((work / f"dist/lib/lib{name}.a").is_file() for name in libraries)
    ):
        return
    work.mkdir(parents=True, exist_ok=True)
    source = work / "ffmpeg-8.1.2"
    if not (source / ".git").is_dir():
        runner(
            [
                "git",
                "clone",
                "--depth=1",
                "-b",
                "n8.1.2",
                "https://github.com/FFmpeg/FFmpeg",
                str(source),
            ],
            check=True,
        )
    runner(
        [
            "container",
            "exec",
            "--workdir",
            "/workspace/apps/mister/target/ffmpeg-minimal/armv7/ffmpeg-8.1.2",
            container,
            "sh",
            "-ec",
            CONFIGURE + " && make -j4 install",
        ],
        check=True,
    )
    stamp.write_text(fingerprint)
