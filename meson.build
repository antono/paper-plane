project(
  'paper-plane',
  'rust',
  version: '0.1.0-beta.6',
  meson_version: '>= 0.59',
)

i18n = import('i18n')
gnome = import('gnome')

base_id = 'app.drey.PaperPlane'

# The minimums of the features of the Rust bindings in Cargo.toml (gtk4-rs
# gnome_49, libadwaita-rs v1_8), checked by build-aux/check-packaging.py
dependency('glib-2.0', version: '>= 2.86')
dependency('gio-2.0', version: '>= 2.86')
dependency('gtk4', version: '>= 4.20')
dependency('libadwaita-1', version: '>= 1.8')
dependency('shumate-1.0', version: '>= 1')
dependency('tdjson', version: '>= 1.8.65')

glib_compile_resources = find_program('glib-compile-resources', required: true)
glib_compile_schemas = find_program('glib-compile-schemas', required: true)
desktop_file_validate = find_program('desktop-file-validate', required: false)
appstream_util = find_program('appstream-util', required: false)
cargo = find_program('cargo', required: true)

version = meson.project_version()

prefix = get_option('prefix')
bindir = prefix / get_option('bindir')
localedir = prefix / get_option('localedir')

datadir = prefix / get_option('datadir')
pkgdatadir = datadir / meson.project_name()
iconsdir = datadir / 'icons'
podir = meson.project_source_root() / 'po'
gettext_package = meson.project_name()

tg_api_id = get_option('tg_api_id')
tg_api_hash = get_option('tg_api_hash')

if get_option('profile') == 'development'
  profile = 'Devel'
  # The revision, when building from a git checkout (not from a tarball or
  # in a sandbox without git)
  vcs_tag = ''
  git = find_program('git', required: false)
  if git.found()
    count = run_command(git, 'rev-list', '--count', 'HEAD', check: false)
    rev = run_command(git, 'rev-parse', '--short', 'HEAD', check: false)
    if count.returncode() == 0 and rev.returncode() == 0
      vcs_tag = 'r@0@.@1@'.format(count.stdout().strip(), rev.stdout().strip())
    endif
  endif
  if vcs_tag == ''
    version_suffix = '-devel'
  else
    version_suffix = '-@0@'.format(vcs_tag)
  endif
  application_id = '@0@.@1@'.format(base_id, profile)
else
  profile = ''
  version_suffix = ''
  application_id = base_id
endif

meson.add_dist_script(
  'build-aux/dist-vendor.sh',
  meson.project_build_root() / 'meson-dist' / meson.project_name() + '-' + version,
  meson.project_source_root()
)

subdir('data')
subdir('po')
subdir('src')

gnome.post_install(
  gtk_update_icon_cache: true,
  glib_compile_schemas: true,
  update_desktop_database: true,
)

# Checks of the packaging files (doap, Flatpak manifest, CI, dependency
# minimums)
test(
  'check-packaging',
  find_program('python3'),
  args: [meson.project_source_root() / 'build-aux' / 'check-packaging.py', meson.project_source_root()],
)
