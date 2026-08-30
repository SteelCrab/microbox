# Dynamic resolution validation

검증일: 2026-08-30  
브랜치: `feat/v0.3-dynamic-resolution`

## Automated checks

다음 검증을 모두 통과했습니다.

```sh
cargo fmt --all -- --check
cargo test --all-targets
cargo test --all-targets -- --ignored --test-threads=1
cargo clippy --all-targets --all-features -- -D warnings
cargo check --manifest-path fuzz/Cargo.toml --all-targets
shellcheck scripts/check-deps.sh scripts/smoke-test.sh
```

실제 Xvfb 통합 테스트는 한 session에서 `320×180 → 800×480 → 427×263 →
1280×720` 순서로 변경했습니다. 각 단계에서 display 크기, RGB frame 크기,
buffer 길이와 application redraw 영역을 검증했습니다.

## Direct execution captures

PTY의 `TIOCSWINSZ` pixel 값을 실행 중 변경하여 native, OCI, Firecrab data-plane
경로를 실제 release binary로 실행했습니다. 각 capture의 payload 길이는 정확히
`width × height × 3` bytes였습니다.

| Runtime | Initial frame | Resized frame | Result |
| --- | ---: | ---: | --- |
| Native | 777×333 / 776,223 bytes | 923×517 / 1,431,573 bytes | pass |
| OCI | 701×389 / 818,067 bytes | 877×541 / 1,423,371 bytes | pass |
| Firecrab agent transport | 733×401 / 881,799 bytes | 911×529 / 1,445,757 bytes | pass |

Native RGB SHA-256:

```text
a9f1b77f85e7fd660f666351719ae03f137f8690a90585f9b9415ac514cf20c3  777x333
a9ff04ba17e65935f63881d0a054586951a3130c2987cabdc6e807d6197d1f8f  923x517
```

초기 구현에서 resize 직후 application redraw 이전의 transitional frame이 먼저
전송되는 현상을 capture 분석으로 발견했습니다. resize가 새 XDamage event를
기다리고 짧은 draw-settle 구간을 거친 뒤 full frame을 보내도록 수정했습니다.
수정 후 native `xeyes`의 non-black bounding box는 `36,32–741,301`에서
`42,50–881,467`로 새 framebuffer와 함께 확장됐으며, 잘림이나 이전 frame 잔상이
없었습니다. OCI와 Firecrab resized capture도 같은 방식으로 육안 확인했습니다.

모든 실행은 `Ctrl-C`에 exit code 0으로 종료됐습니다. 종료 후 microbox session
record, 테스트 Docker container, private Xvfb와 application child가 남지 않은 것을
확인했습니다.

Firecrab 검증은 실제 guest image와 TCP port mapping을 사용한 GUI data-plane
검증입니다. Firecrab VM 생성·network 선택·image import/delete 같은 control-plane
정책은 [Firecrab transport guide](firecrab.md)에 명시된 별도 운영 경계입니다.
