# Performance baseline

측정일: 2026-08-30  
브랜치: `feat/m3-rendering-efficiency`  
환경: Linux x86_64, Xvfb 640×360x24, `xeyes`, release build, pseudo-TTY

## Idle session

```sh
cargo build --release
/usr/bin/time -f 'elapsed=%e user=%U system=%S cpu=%P max_rss_kb=%M' \
  script -qefc \
  'timeout -s INT -k 2 5 target/release/microbox run xeyes --fps 30 --stats' \
  /dev/null >/dev/null
```

관측값:

```text
elapsed=5.12 user=0.06 system=0.05 cpu=2% max_rss_kb=87024
```

별도 2초 render counter 표본:

```text
polls=56, captured=2, full=2, tile_frames=0, tiles=0, unchanged=0, skipped=54
```

XDamage가 idle frame capture를 억제하여 초기 목표인 CPU 5% 미만을 충족했습니다.
수치는 개발 환경의 기준값이며 CI 또는 다른 terminal emulator의 보장값이 아닙니다.

## Rendering policy

- frame clock은 1–60 FPS이며 기본값은 30 FPS입니다.
- overdue tick을 누적하지 않고 다음 deadline을 현재 시각 기준으로 다시 잡습니다.
- XDamage event가 없으면 X11 capture와 terminal write를 모두 건너뜁니다.
- 변경된 64px tile 면적이 전체의 35% 미만이면 tile overlay를 전송합니다.
- 임계값 이상이거나 terminal resize가 발생하면 full frame으로 재동기화합니다.
- MIT-SHM 1.2가 있으면 shared-memory capture를 사용하고, 없으면 `GetImage`로
  fallback합니다.
