# flatpak-builder with the GNOME 47 runtime and SDK, for ci/gate/crossui/linux-flatpak.sh.
# Runs privileged (bubblewrap needs user namespaces and mounts).
FROM ubuntu:24.04
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends \
      flatpak flatpak-builder ca-certificates xvfb xauth x11-utils dbus dbus-x11 netpbm \
    && rm -rf /var/lib/apt/lists/*
RUN flatpak remote-add --system --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo \
    && flatpak install --system -y --noninteractive flathub org.gnome.Platform//47 org.gnome.Sdk//47
# xwd for the launch screenshot (after the runtime layer, which takes a while to download).
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends x11-apps && rm -rf /var/lib/apt/lists/*
