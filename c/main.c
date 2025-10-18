/* Copyright (c) 2025 Ari Rios <aririos0719@gmail.com */
/* LICENSE: MIT */
/* Credit to Isty001 for dotenv.c: https://github.com/Isty001/dotenv-c/ */

#include "main.h"

bool need_free_music_dir = false;

bool int_in_arr(int needle, int *haystack, size_t len) {
  for (size_t i = 0; i < len; i++) {
    if (haystack[i] == needle) {
      return true;
    }
  }
  return false;
}

int str_pos_in_arr(char *needle, char **haystack, size_t len) {
  for (size_t i = 0; i < len; i++) {
    if (strcmp(haystack[i], needle) == 0) {
      return i;
    }
  }
  return -1;
}

char *replace_all(char *str, const char *substr, const char *replacement) {
  if (strcmp(str, "") == 0 || strcmp(substr, "") == 0) {
    fprintf(stderr, "Empty string or substring not supported");
    return NULL;
  }
  char *pos = strstr(str, substr);
  if (pos == NULL) {
    return str;
  }
  char *last;
  char *out = calloc(strlen(str) + 1, sizeof(char));
  if (out == NULL) {
    oom_message("replace_all out malloc");
    return NULL;
  }
  char *new_out;
  ptrdiff_t init_diff = pos - str + 1;
  snprintf(out, init_diff, "%s", str);
  while (pos) {
    int new_len = strlen(out) + strlen(replacement) + 1;
    new_out = realloc(out, new_len);
    if (new_out == NULL) {
      free(out);
      oom_message("replace_all new_out malloc");
      return NULL;
    }
    out = new_out;
    strcat(out, replacement);
    last = pos + strlen(substr);
    pos = strstr(last, substr);
    if (pos) {
      ptrdiff_t next_diff = pos - last;
      new_out = realloc(out, new_len + next_diff);
      if (new_out == NULL) {
        free(out);
        oom_message("replace_all if pos realloc");
        return NULL;
      }
      out = new_out;
      strncat(out, last, next_diff);
    }
    else {
      new_out = realloc(out, new_len + strlen(last));
      if (new_out == NULL) {
        free(out);
        oom_message("replace_all else realloc");
        return NULL;
      }
      out = new_out;
      strcat(out, last);
    }
  }

  return out;
}

double euclidean_distance(double *a, double *b, int dimensions) {
  double sum = 0.0;
  for (int i = 0; i < dimensions; i++) {
    sum += pow(a[i] - b[i], 2);
  }
  return sqrt(sum);
}

int oom_message(char *loc, ...) {
  // credit to https://stackoverflow.com/a/66095977
  va_list args;
  va_start(args, loc);
  int len = vsnprintf(NULL, 0, loc, args);
  va_end(args);
  if (len < 0) {
    return 2;
  }
  char msg[len + 1];
  va_start(args, loc);
  vsnprintf(msg, len + 1, loc, args);
  va_end(args);

  fprintf(stderr, "Out of memory at %s\n", msg);
  return 1;
}

void maybe_free_music_dir(char *music_dir) {
  if (need_free_music_dir) {
    free(music_dir);
  }
}

int init_dynamic_songs_array(dynamic_songs_array *arr, size_t init_size) {
  if (init_size <= 0) {
    fprintf(stderr, "init_size must be greater than zero\n");
    return 1;
  }
  arr->songs = malloc(init_size * sizeof(db_song *));
  if (arr->songs == NULL) {
    return oom_message("init_dynamic_songs_array");
  }
  arr->length = 0;
  arr->capacity = init_size;
  return 0;
}

int push_dynamic_songs_array(dynamic_songs_array *arr, db_song *element) {
  if (arr->length == arr->capacity) {
    arr->capacity *= 2;
    arr->songs = realloc(arr->songs, arr->capacity * sizeof(db_song *));
    if (arr->songs == NULL) {
      return oom_message("push_dynamic_char_array for new capacity %d", arr->capacity);
    }
  }
  arr->songs[arr->length++] = element;
  return 0;
}

void free_dynamic_songs_array(dynamic_songs_array *arr) {
  for (int i = 0; i < arr->capacity; i++) {
    free(arr->songs[i]->path);
    free(arr->songs[i]);
  }
  free(arr->songs);
}

bool strtol_err_wrap(char *str, int *output) {
  char *end_ptr;
  errno = 0;
  *output = strtol(str, &end_ptr, 10);
  if (errno == ERANGE || *end_ptr != '\0' || end_ptr == str) {
    fprintf(stderr, "strtol_err_wrap failed with argument %s", str);
    return false;
  }
  return true;
}

bool get_int_by_column_name(char *col_name, int argc, char **argv, char **col_names, int *val) {
  int col_pos = str_pos_in_arr(col_name, col_names, argc);
  if (col_pos == -1) {
    fprintf(stderr, "%s column not found at get_int_by_column_name\n", col_name);
    return false;
  }
  return strtol_err_wrap(argv[col_pos], val);
}

void euclidean_distance_callback(void *center, va_alist alist) {
  bliss_analysis *center_ptr = center;
  va_start_ptr(alist, int);
  bliss_analysis **a_ptr = va_arg_ptr(alist, bliss_analysis **);
  if (a_ptr == NULL) {
    fprintf(stderr, "Not enough arguments provided to euclidean_distance_callback: expected 2, got 0\n");
    exit(1);
  }
  bliss_analysis **b_ptr = va_arg_ptr(alist, bliss_analysis **);
  if (b_ptr == NULL) {
    fprintf(stderr, "Not enough arguments provided to euclidean_distance_callback: expected 2, got 1\n");
    exit(1);
  }
  bliss_analysis *a = *a_ptr;
  bliss_analysis *b = *b_ptr;
  double distance_a = euclidean_distance(center_ptr->features, a->features, NUM_FEATURES);
  double distance_b = euclidean_distance(center_ptr->features, b->features, NUM_FEATURES);
  double diff = distance_a - distance_b;
  if (fabs(diff) < 0.05) { // TODO: do some unscientific tests to determine what the epsilon should be
    va_return_ptr(alist, int, 0);
  }
  else {
    va_return_ptr(alist, int, diff > 0 ? 1 : -1);
  }
}

int song_id_callback(void *song_id, int argc, char **argv, char **col_name) {
  int *song_id_ptr = song_id;
  return !get_int_by_column_name("id", argc, argv, col_name, song_id_ptr);
}

int song_query_callback(void *controller_ptr, int argc, char **argv, char **col_name) {
  song_query_controller *controller = controller_ptr;
  db_song *song = controller->song;
  int path_col_pos = str_pos_in_arr("path", col_name, argc);
  if (path_col_pos == -1) {
    fprintf(stderr, "path column not found at song_query_callback\n");
    return 1;
  }
  int len = strlen(argv[path_col_pos]) + 1;
  song->path = malloc(len);
  if (song->path == NULL) {
    return oom_message("song_query_callback malloc\n");
  }
  if (strstr(argv[path_col_pos], controller->music_dir) == argv[path_col_pos]) {
    strcpy(song->path, argv[path_col_pos] + strlen(controller->music_dir));
  }
  else {
    fprintf(stderr, "Failed to remove music_dir prefix at song_query_callback\n");
    return 1;
  }
  return 0;
}

int bliss_library_callback(void *controller_ptr, int argc, char **argv, char **col_name) {
  library_controller *controller = controller_ptr;
  int song_id = 0;
  if (!get_int_by_column_name("id", argc, argv, col_name, &song_id)) {
    return 1;
  }
  bliss_analysis *(*library_ptr)[controller->max_song_id + 1] = controller->library;
  bliss_analysis *analysis = (*library_ptr)[song_id];
  analysis->song_id = song_id;
  if (get_bliss_analysis_features(controller->bliss_db, song_id, analysis) != 0) {
    fprintf(stderr, "get_bliss_analysis_features failed for song_id %d at bliss_library_callback\n", song_id);
    free(analysis);
    return 1;
  }
  return 0;
}

int bliss_analysis_callback(void *analysis, int argc, char **argv, char **col_name) {
  char *end_ptr;
  bliss_analysis *analysis_ptr = analysis;
  int feature_index_index = 0;
  if (!get_int_by_column_name("feature_index", argc, argv, col_name, &feature_index_index)) {
    return 1;
  };
  int feature_pos = str_pos_in_arr("feature", col_name, argc);
  if (feature_pos == -1) {
    fprintf(stderr, "feature column not found at bliss_analysis_callback\n");
    return 1;
  }
  errno = 0;
  double feature = strtod(argv[feature_pos], &end_ptr);
  if (errno == ERANGE || *end_ptr != '\0' || end_ptr == argv[feature_pos]) {
    fprintf(stderr, "strtol failed at bliss_analysis_callback\n");
    return 1;
  }
  analysis_ptr->features[feature_index_index] = feature;
  return 0;
}

int get_song_id(db_song *song, sqlite3 *bliss_db, char *mpd_music_dir) {
  char *z_err = 0;
  char *sql_header = "select * from song where path = ";
  int len = strlen(sql_header) + strlen(mpd_music_dir) +
            strlen(song->path) +
            4; // 4 = two quotes, semicolon, and a null
  char *sql_query = malloc(len);
  if (sql_query == NULL) {
    return oom_message("get_song_id malloc");
  }
  snprintf(sql_query, len, "%s'%s%s';", sql_header, mpd_music_dir, song->path);
  int song_id = 0;
  int bliss_rc = sqlite3_exec(bliss_db, sql_query, song_id_callback, &song_id, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at get_song_id: %s\n", z_err);
    free(z_err);
    free(sql_query);
    return 0;
  }
  free(sql_query);
  return song_id;
}

char *query_builder(char *header, int variable, int footer) {
  int len = strlen(header) + snprintf(NULL, 0, "%d", variable) + footer;
  char *query = malloc(len);
  if (query == NULL) {
    oom_message("query_builder\n");
    return NULL;
  }
  snprintf(query, len, "%s%d;", header, variable);
  return query;
}

void db_songs_free(size_t len, db_song *songs[len], size_t start) {
  if (start > len) {
    fprintf(stderr, "starting position after array end at db_songs_free\n");
    exit(1);
  }
  for (int i = start; i < len; i++) {
    if (songs[i] != NULL && songs[i]->path != NULL) {
      free(songs[i]->path);
      free(songs[i]);
    }
  }
}

void library_songs_free(size_t len, bliss_analysis *(*library)[len], size_t start) {
  if (start > len) {
    fprintf(stderr, "starting position after array end at db_songs_free\n");
    exit(1);
  }
  for (int i = start; i < len; i++) {
    if ((*library)[i] != NULL) {
      free((*library)[i]);
    }
  }
}

int populate_song(db_song *song, char *music_dir, int argc, char **argv, char **col_name) {
  int song_id = 0;
  if (!get_int_by_column_name("id", argc, argv, col_name, &song_id)) {
    return 1;
  }
  song->song_id = song_id;
  int path_col_pos = str_pos_in_arr("path", col_name, argc);
  if (path_col_pos == -1) {
    fprintf(stderr, "path column not found at populate_song\n");
    return 1;
  }
  song->path = malloc(strlen(argv[path_col_pos]) - strlen(music_dir) + 1);
  if (song->path == NULL) {
    return oom_message("populate_song song->path malloc\n");
  }
  if (strstr(argv[path_col_pos], music_dir) == argv[path_col_pos]) {
    strcpy(song->path, argv[path_col_pos] + strlen(music_dir));
  }
  else {
    free(song->path);
    fprintf(stderr, "Failed to remove music_dir prefix at populate_song\n");
    return 1;
  }
  return 0;
}

int random_query_callback(void *controller_ptr, int argc, char **argv, char **col_name) {
  random_query_controller *controller = controller_ptr;
  db_song *song = malloc(sizeof(db_song));
  if (song == NULL) {
    return oom_message("random_query_callback db_song malloc");
  }
  if (populate_song(song, controller->music_dir, argc, argv, col_name) != 0) {
    fprintf(stderr, "Failed to populate song at random_query_callback\n");
    free(song);
    return 1;
  }
  controller->songs[controller->i] = song;
  controller->i++;
  return 0;
}

int glob_query_callback(void *controller_ptr, int argc, char **argv, char **col_name) {
  glob_query_controller *controller = controller_ptr;
  db_song *song = malloc(sizeof(db_song));
  if (song == NULL) {
    return oom_message("random_query_callback db_song malloc");
  }
  if (populate_song(song, controller->music_dir, argc, argv, col_name) != 0) {
    fprintf(stderr, "Failed to populate song at glob_query_callback\n");
    free(song);
    return 1;
  }
  return push_dynamic_songs_array(controller->songs, song);
}

int random_playlist(struct mpd_connection *conn, sqlite3 *bliss_db, size_t playlist_len, char *music_dir, char *playlist_name) {
  char *z_err = 0;
  srand((unsigned)time(NULL));
  printf("Creating random playlist of length %jd...\n", playlist_len);
  db_song *songs[playlist_len];
  random_query_controller controller = { .i = 0, .music_dir = music_dir, .songs = songs };
  char *random_query = query_builder("select * from song order by random() limit ", playlist_len, 2);
  if (random_query == NULL) {
    return 1;
  }
  int bliss_rc = sqlite3_exec(bliss_db, random_query, random_query_callback, &controller, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at random_playlist: %s\n", z_err);
    free(random_query);
    free(z_err);
    return 1;
  }
  free(random_query);

  bool rm_rc = mpd_run_rm(conn, playlist_name);
  if (!rm_rc) {
    enum mpd_error err = mpd_connection_get_error(conn);
    if (err == MPD_ERROR_SERVER) {
      enum mpd_server_error server_err = mpd_connection_get_server_error(conn);
      if (server_err != MPD_SERVER_ERROR_NO_EXIST) {
        fprintf(stderr, "mpd_run_rm server error code: %d", server_err);
        db_songs_free(playlist_len, songs, 0);
        return 1;
      }
    }
    else {
      printf("mpd_run_rm error: %d\n", mpd_connection_get_error(conn));
      db_songs_free(playlist_len, songs, 0);
      return 1;
    }
  }
  for (int i = 0; i < playlist_len; i++) {
    bool playlist_add_rc = mpd_run_playlist_add(conn, playlist_name, songs[i]->path);
    if (!playlist_add_rc) {
      enum mpd_error err = mpd_connection_get_error(conn);
      if (err == MPD_ERROR_SERVER) {
        fprintf(stderr, "mpd_run_playlist_add server error code: %d\n", mpd_connection_get_server_error(conn));
        db_songs_free(playlist_len, songs, i);
        return 1;
      }
      fprintf(stderr, "mpd_run_playlist_add error: %d\n", mpd_connection_get_error(conn));
      db_songs_free(playlist_len, songs, i);
      return 1;
    }
    free(songs[i]->path);
    free(songs[i]);
  }
  return 0;
}

int get_bliss_analysis_features(sqlite3 *bliss_db, int song_id, bliss_analysis *analysis) {
  char *z_err = 0;
  char *bliss_analysis_query = query_builder("select * from feature where song_id = ", song_id, 2);
  if (bliss_analysis_query == NULL) {
    return 1;
  }
  int bliss_rc = sqlite3_exec(bliss_db, bliss_analysis_query, bliss_analysis_callback, analysis, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at get_bliss_analysis_features: %s\n", z_err);
    free(z_err);
    free(bliss_analysis_query);
    return 1;
  }
  free(bliss_analysis_query);
  return 0;
}

int get_bliss_library(sqlite3 *bliss_db, int max_song_id, bliss_analysis *(*library)[max_song_id + 1]) {
  char *z_err = 0;
  library_controller library_controller = {
    .library = library,
    .bliss_db = bliss_db,
    .max_song_id = max_song_id
  };
  char *bliss_library_query = "select * from song;";
  int bliss_rc = sqlite3_exec(bliss_db, bliss_library_query, bliss_library_callback, &library_controller, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at get_bliss_library: %s\n", z_err);
    free(z_err);
    return 1;
  }
  return 0;
}

int max_song_id_callback(void *max_song_id, int argc, char **argv, char **col_name) {
  int *max_song_id_ptr = max_song_id;
  if (!get_int_by_column_name("max(id)", argc, argv, col_name, max_song_id_ptr)) {
    return 1;
  };
  return 0;
}

bool get_max_song_id(sqlite3 *bliss_db, int *max_song_id) {
  char *z_err;
  char *max_song_id_query = "select max(id) from song;";
  int bliss_rc = sqlite3_exec(bliss_db, max_song_id_query, max_song_id_callback, max_song_id, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at get_max_song_id: %s\n", z_err);
    free(z_err);
    return false;
  }
  return true;
}

int playlist_from_song_id(sqlite3 *bliss_db, char *music_dir, int base_song_id, struct mpd_connection *conn, char *playlist_name, int playlist_len) {
  printf("Making playlist from song_id %d\n", base_song_id);
  int max_song_id = 0;
  if (!get_max_song_id(bliss_db, &max_song_id)) {
    fprintf(stderr, "Failed to get max song_id\n");
    maybe_free_music_dir(music_dir);
    return 1;
  }
  bliss_analysis *(*library)[max_song_id + 1] = calloc(1, sizeof(bliss_analysis * [max_song_id + 1]));
  if (library == NULL) {
    maybe_free_music_dir(music_dir);
    return oom_message("library malloc\n");
  }
  for (int i = 0; i <= max_song_id; i++) {
    bliss_analysis *analysis = calloc(1, sizeof(bliss_analysis));
    if (analysis == NULL) {
      maybe_free_music_dir(music_dir);
      library_songs_free(max_song_id, library, 0);
      free(library);
      return oom_message("analysis malloc for i %d", i);
    }
    (*library)[i] = analysis;
  }
  get_bliss_library(bliss_db, max_song_id, library);
  bliss_analysis *base_song_analysis = malloc(sizeof(bliss_analysis));
  base_song_analysis->song_id = base_song_id;
  get_bliss_analysis_features(bliss_db, base_song_id, base_song_analysis);
  callback_t sort_callback = alloc_callback(euclidean_distance_callback, base_song_analysis);
  qsort(*library, max_song_id + 1, sizeof(bliss_analysis *), (__compar_fn_t)sort_callback);
  free(base_song_analysis);
  bool rm_rc = mpd_run_rm(conn, playlist_name);
  if (!rm_rc) {
    enum mpd_error err = mpd_connection_get_error(conn);
    if (err == MPD_ERROR_SERVER) {
      enum mpd_server_error server_err = mpd_connection_get_server_error(conn);
      if (server_err != MPD_SERVER_ERROR_NO_EXIST) {
        fprintf(stderr, "mpd_run_rm server error code: %d", server_err);
        maybe_free_music_dir(music_dir);
        library_songs_free(max_song_id + 1, library, 0);
        free(library);
        return 1;
      }
    }
    else {
      fprintf(stderr, "mpd_run_rm error: %d\n", mpd_connection_get_error(conn));
      maybe_free_music_dir(music_dir);
      library_songs_free(max_song_id + 1, library, 0);
      free(library);
      return 1;
    }
  }
  char *z_err = 0;
  for (int i = 0; i < playlist_len; i++) {
    char *song_query = query_builder("select * from song where id = ", (*library)[i]->song_id, 2);
    if (song_query == NULL) {
      maybe_free_music_dir(music_dir);
      return 1;
    }
    db_song song = { .song_id = (*library)[i]->song_id };
    song_query_controller controller = {
      .music_dir = music_dir,
      .song = &song
    };
    int bliss_rc = sqlite3_exec(bliss_db, song_query, song_query_callback, &controller, &z_err);
    if (bliss_rc != SQLITE_OK) {
      fprintf(stderr, "sqlite exec error at main: %s\n", z_err);
      free(z_err);
      free(song.path);
      free(song_query);
      maybe_free_music_dir(music_dir);
      library_songs_free(max_song_id + 1, library, 0);
      free(library);
      return 1;
    }
    free(song_query);
    bool playlist_add_rc = mpd_run_playlist_add(conn, playlist_name, song.path);
    if (!playlist_add_rc) {
      enum mpd_error err = mpd_connection_get_error(conn);
      if (err == MPD_ERROR_SERVER) {
        enum mpd_server_error server_err = mpd_connection_get_server_error(conn);
        if (server_err == MPD_SERVER_ERROR_NO_EXIST) {
          fprintf(stderr, "mpd_run_playlist_add server error code: %d\n", server_err);
          fprintf(stderr, "This probably means you need to touch ~/.mpd/playlists/%s.m3u (or the equivalent for your setup) on the server running MPD\n", playlist_name);
          maybe_free_music_dir(music_dir);
          free(song.path);
          library_songs_free(max_song_id + 1, library, 0);
          free(library);
          return 1;
        }
        fprintf(stderr, "mpd_run_playlist_add server error code: %d\n", server_err);
        maybe_free_music_dir(music_dir);
        free(song.path);
        library_songs_free(max_song_id + 1, library, 0);
        free(library);
        return 1;
      }
      fprintf(stderr, "mpd_run_playlist_add error: %d\n", mpd_connection_get_error(conn));
      maybe_free_music_dir(music_dir);
      free(song.path);
      library_songs_free(max_song_id + 1, library, 0);
      free(library);
      return 1;
    }
    free(song.path);
  }
  library_songs_free(max_song_id + 1, library, 0);
  free(library);
  return 0;
}

int main(int argc, char **argv) {
  sqlite3 *bliss_db;

  if (env_load(".", false) != 0) {
    fprintf(stderr, "Failed to load .env file, continuing...\n");
  }

  char *mpd_host = getenv("MPD_HOST");
  char *mpd_port_string = getenv("MPD_PORT");
  char *mpd_password = getenv("MPD_PASSWORD");
  char *blissify_password = getenv("BLISSIFY_PASSWORD");
  char *bliss_db_path = getenv("BLISS_DB");
  char *base_song_id_string = NULL;
  char *song_glob = NULL;
  char *music_dir = getenv("MUSIC_DIR");
  int run_blissify_update = 1;
  char *playlist_len_string = NULL;
  char *playlist_name = NULL;

  while (1) {
    struct option long_opts[] = {
      { "help", no_argument, 0, 'h' },
      { "song-id", required_argument, 0, 's' },
      { "song-glob", required_argument, 0, 'g' },
      { "run-blissify-update", required_argument, &run_blissify_update, 'r' },
      { "length", required_argument, 0, 'l' },
      { "playlist-name", required_argument, 0, 'n' },
      { 0, 0, 0, 0 }
    };
    int opt_index = 0;
    int c = getopt_long(argc, argv, "hs:g:r:l:n:", long_opts, &opt_index);
    if (c == -1) {
      break;
    }
    switch (c) {
      case 0:
        fprintf(stderr, "Unknown getopt_long option %c\n", c);
        break;
      case 'h':
        printf("worf 0.0.0\n"
               "aririos\n"
               "\n"
               "Usage: \n"
               "\nworf\n"
               "\n"
               "Generate playlists using an existing `blissify` database. With option --song-id, playlist is based on Euclidean distance from that song; with --song-glob, will look up song from database based on a glob pattern (not yet implemented); otherwise, playlist is randomly generated."

        );
        return 0;
      case 's':
        base_song_id_string = optarg;
        break;
      case 'g':
        song_glob = optarg;
        break;
      case 'r':
        // sets flag, don't need to do anything
        break;
      case 'l':
        playlist_len_string = optarg;
        break;
      case 'n':
        playlist_name = optarg;
        break;
      case '?':
        break;
      default:
        fprintf(stderr, "Unknown getopt_long error\n");
        return 1;
    }
  }

  if (optind < argc) {
    fprintf(stderr, "Extra unrecognized arguments: ");
    while (optind < argc) {
      fprintf(stderr, "%s ", argv[optind++]);
    }
    putchar('\n');
  }

  if (base_song_id_string != NULL && song_glob != NULL) {
    fprintf(stderr, "Options --song-id and --song-glob cannot be used simultaneously\n");
    return 1;
  }
  int mpd_port = 0;
  if (mpd_port_string != NULL && !strtol_err_wrap(mpd_port_string, &mpd_port)) {
    printf("Could not convert %s to int, using default MPD port\n", mpd_port_string);
  }
  struct mpd_connection *conn = mpd_connection_new(mpd_host, mpd_port, 300000);
  if (conn == NULL) {
    return oom_message("mpd_connection_new\n");
  }
  mpd_connection_set_keepalive(conn, true);

  if (mpd_password) {
    if (!mpd_run_password(conn, mpd_password)) {
      fprintf(stderr, "Bad password\n");
      mpd_connection_free(conn);
      return 1;
    }
  }

  if (bliss_db_path == NULL) {
    fprintf(stderr, "No bliss sqlite db location specified, use environment variable BLISS_DB\n");
    return 1;
  }

  if (music_dir == NULL) {
    fprintf(stderr, "No music directory specified, use environment variable MUSIC_DIR\n");
    return 1;
  }

  if (music_dir[strlen(music_dir)] != '/') { // TODO: windows :(
    int len = strlen(music_dir);
    char *old_music_dir = malloc(len + 1);
    if (old_music_dir == NULL) {
      mpd_connection_free(conn);
      return oom_message("main old_music_dir malloc\n");
    }
    strcpy(old_music_dir, music_dir);
    music_dir = malloc(len + 2);
    if (music_dir == NULL) {
      free(old_music_dir);
      mpd_connection_free(conn);
      return oom_message("main music_dir realloc\n");
    }
    need_free_music_dir = true;
    snprintf(music_dir, len + 2, "%s/", old_music_dir);
    free(old_music_dir);
  }

  int bliss_rc = sqlite3_open(bliss_db_path, &bliss_db);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite error at open bliss_db: %s\n", sqlite3_errmsg(bliss_db));
    mpd_connection_free(conn);
    maybe_free_music_dir(music_dir);
    return 1;
  }

  if (mpd_connection_get_error(conn) != MPD_ERROR_SUCCESS) {
    fprintf(stderr, "mpd connection error at init: %s\n", mpd_connection_get_error_message(conn));
    mpd_connection_free(conn);
    int bliss_db_close_status = sqlite3_close(bliss_db);
    if (bliss_db_close_status != SQLITE_OK) {
      fprintf(stderr, "Failed to close bliss_db with error code %d\n", bliss_db_close_status);
    }
    maybe_free_music_dir(music_dir);
    return 1;
  }

  if (run_blissify_update) {
    char blissify_cmd[256];
    // FIXME: passing untrusted data to system() here
    if (blissify_password) {
      snprintf(blissify_cmd, 255, "MPD_HOST='%s@127.0.0.1' MPD_PORT=6600 blissify update", blissify_password); // TODO: option to provide host and port of device hosting blissify, otherwise get host and port programmatically
    }
    else {
      snprintf(blissify_cmd, 255, "MPD_PORT=6600 blissify update"); // TODO: option to provide host and port of device hosting blissify, otherwise get host and port programmatically
    }
    printf("Running `blissify update`...\n");
    int blissify_rc = system(blissify_cmd);
    if (blissify_rc != 0) {
      fprintf(stderr, "`blissify update` failed with exit code %d", blissify_rc);
      mpd_connection_free(conn);
      maybe_free_music_dir(music_dir);
      return 1;
    }
  }

  int playlist_len = 50;
  if (playlist_len_string == NULL) {
    printf("No playlist length provided, defaulting to 50...\n");
  }
  else {
    if (!strtol_err_wrap(playlist_len_string, &playlist_len)) {
      fprintf(stderr, "Failed to convert string %s to playlist length\n", playlist_len_string);
      maybe_free_music_dir(music_dir);
      mpd_connection_free(conn);
      return 1;
    }
  }

  if (playlist_name == NULL) {
    printf("No playlist name provided, defaulting to \"bliss-playlist\"\n");
    playlist_name = "bliss-playlist";
  }

  if (song_glob != NULL) {
    char *z_err = 0;
    int base_song_id_int = 0;
    char *song_glob_query_format = "select * from song where lower(path) glob '%s';";
    int query_len = strlen(song_glob_query_format) - 2 + strlen(song_glob) + 1;
    char *song_glob_query = malloc(query_len);
    snprintf(song_glob_query, query_len, song_glob_query_format, song_glob);
    dynamic_songs_array songs;
    init_dynamic_songs_array(&songs, 1);
    glob_query_controller controller = {
      .music_dir = music_dir,
      .songs = &songs
    };
    int bliss_rc = sqlite3_exec(bliss_db, song_glob_query, glob_query_callback, &controller, &z_err);
    if (bliss_rc != SQLITE_OK) {
      fprintf(stderr, "Song glob query failed with error %s", z_err);
      free(z_err);
      free(song_glob_query);
      mpd_connection_free(conn);
      return 1;
    }
    printf("Search returned:\n");
    for (int i = 0; i < songs.length; i++) {
      printf("%d: %s\n", i, songs.songs[i]->path);
    }
    while (1) {
      printf("Enter your selection:\n");
      int selection = -1;
      scanf("%d", &selection);
      if (selection > -1 && selection < songs.length) {
        base_song_id_int = songs.songs[selection]->song_id;
        break;
      }
      else {
        printf("Invalid selection\n");
      }
    }
    free_dynamic_songs_array(&songs);
    int playlist_rc = playlist_from_song_id(bliss_db, music_dir, base_song_id_int, conn, playlist_name, playlist_len);
    if (playlist_rc != 0) {
      fprintf(stderr, "Creating playlist from song_id failed with error code %d\n", playlist_rc);
    }
    free(song_glob_query);
  }
  else if (base_song_id_string != NULL) {
    int base_song_id_int = 0;
    strtol_err_wrap(base_song_id_string, &base_song_id_int);
    int playlist_rc = playlist_from_song_id(bliss_db, music_dir, base_song_id_int, conn, playlist_name, playlist_len);
    if (playlist_rc != 0) {
      fprintf(stderr, "Creating playlist from song_id failed with error code %d\n", playlist_rc);
    }
  }
  else {
    int random_rc = random_playlist(conn, bliss_db, playlist_len, music_dir, playlist_name);
    if (random_rc != 0) {
      fprintf(stderr, "Creating random playlist failed with error code %d\n", random_rc);
    }
  }
  printf("Closing...\n");
  maybe_free_music_dir(music_dir);
  mpd_connection_free(conn);
  int bliss_db_close_status = sqlite3_close(bliss_db);
  if (bliss_db_close_status != SQLITE_OK) {
    fprintf(stderr, "Failed to close bliss_db with error code %d\n", bliss_db_close_status);
  }
  return 0;
}
