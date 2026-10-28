FROM archlinux:base

RUN pacman -Syu --noconfirm --needed \
        base-devel rust cargo git pkgconf \
        wayland libxkbcommon \
        sway swaybg xorg-xwayland \
        mesa vulkan-icd-loader vulkan-swrast vulkan-tools llvm seatd \
    && pacman -Scc --noconfirm \
    && setcap -r /usr/sbin/sway

ENV WLR_BACKENDS=headless
ENV WLR_RENDERER=pixman
ENV WLR_LIBINPUT_NO_DEVICES=1
ENV XDG_RUNTIME_DIR=/tmp/wyrd-test-runtime
ENV LIBGL_ALWAYS_SOFTWARE=1
ENV WGPU_BACKEND=gl

WORKDIR /work
CMD ["/bin/bash"]
