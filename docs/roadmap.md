# micro-gui 구현 로드맵

## M0 — Terminal rendering foundation (현재)

- [x] Rust 프로젝트 기본 구조
- [x] TTY/terminal 진단 명령
- [x] Kitty Graphics RGB24 전송과 payload chunking
- [x] 생성 frame 출력 demo
- [x] frame buffer 검증과 dirty-tile 판별
- [x] 입력 좌표 변환 모델
- [x] session 상태 모델

검증:

```sh
cargo test
cargo run -- doctor
cargo run -- demo
```

## M1 — X11 capture PoC

- [ ] 사용 가능한 private display 번호 할당
- [ ] Xvfb 실행과 readiness timeout
- [ ] argv 기반 애플리케이션 실행
- [ ] 최상위 window 탐색 및 root 크기에 맞춤
- [ ] X11 `GetImage` capture
- [ ] 10 FPS 전체 frame 출력
- [ ] 종료 시 Xvfb와 application process group 정리

완료 조건: Kitty 호환 터미널에서 `micro-gui run xeyes` 화면이 지속적으로
갱신됩니다. 아직 입력은 필요하지 않습니다.

## M2 — Interactive session

- [ ] terminal raw/alternate-screen guard
- [ ] Kitty keyboard event decoder
- [ ] 일반 key event fallback
- [ ] SGR mouse press/release/move decoder
- [ ] XTEST key/button/motion 주입
- [ ] `SIGWINCH` 기반 resize
- [ ] `Ctrl-C`와 비정상 종료 복구

완료 조건: `xeyes`의 시선이 pointer를 따라가고 GTK Demo의 button과 text field를
조작할 수 있습니다.

## M3 — Rendering efficiency

- [ ] render loop backpressure
- [ ] dirty tile update 연결
- [ ] 변경 면적 기반 full/tile 선택
- [ ] 목표 frame rate 옵션과 CPU 측정
- [ ] MIT-SHM capture fast path

초기 성능 목표:

- idle 상태 CPU 5% 미만(개발 기준 장비에서 측정 조건과 함께 기록)
- 기본 목표 30 FPS, 느린 연결에서는 최신 frame 우선
- 입력 event 순서 보존

## M4 — v0.1 hardening

- [ ] `xeyes`, GTK Demo, Leafpad smoke test
- [ ] terminal disconnect와 broken pipe test
- [ ] child crash와 Xvfb crash test
- [ ] frame allocation 상한과 fuzz 가능한 decoder 분리
- [ ] 설치 문서와 demo 녹화
- [ ] v0.1 release checklist

## v0.1 이후

1. Alpine/OCI application image backend
2. Firecrab MicroVM backend
3. Wayland/Weston backend
4. background session daemon과 `ps`/`stop`
5. clipboard, audio, 고급 keyboard/IME
