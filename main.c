#include <assert.h>
#include <errno.h>
#include <getopt.h>
#include <mpd/client.h>
#include <sqlite3.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <stdarg.h>
#include <math.h>
#include "isotree/include/isotree_c.h"
#include <callback.h>
#define NUM_FEATURES 20

typedef struct {
  char *path;
  int song_id;
} db_song;

enum BLISS_FEATURES {
  TEMPO,
  ZERO_CROSSING_RATE,
  MEAN_SPECTRAL_CENTROID,
  STD_DEV_SPECTRAL_CENTROID,
  MEAN_SPECTRAL_ROLLOFF,
  STD_DEV_SPECTRAL_ROLLOFF,
  MEAN_SPECTRAL_FLATNESS,
  STD_DEV_SPECTRAL_FLATNESS,
  MEAN_LOUDNESS,
  STD_DEV_LOUDNESS,
  CHROMA_INTERVAL_ONE,
  CHROMA_INTERVAL_TWO,
  CHROMA_INTERVAL_THREE,
  CHROMA_INTERVAL_FOUR,
  CHROMA_INTERVAL_FIVE,
  CHROMA_INTERVAL_SIX,
  CHROMA_INTERVAL_SEVEN,
  CHROMA_INTERVAL_EIGHT,
  CHROMA_INTERVAL_NINE,
  CHROMA_INTERVAL_TEN
};

typedef struct {
  int song_id;
  double features[NUM_FEATURES];
} bliss_analysis;

typedef struct {
  sqlite3 *bliss_db;
  int max_song_id;
  bliss_analysis *(*library)[];
} library_controller;

typedef struct {
  int i;
  char *music_dir;
  db_song **songs;
} random_query_controller;

bool int_in_arr(int needle, int *haystack, size_t len);
int str_pos_in_arr(char *needle, char **haystack, size_t len);
char *replace_all(char *str, const char *substr, const char *replacement);
double euclidean_distance(double *a, double *b, int dimensions);
void euclidean_distance_callback(void *center, va_alist alist);
bool strtol_err_wrap(char *str, int *output);
bool get_int_by_column_name(char *col_name, int argc, char **argv, char **col_names, int *val);
int song_id_callback(void *song_id, int argc, char **argv, char **col_name);
int song_query_callback(void *song_ptr, int argc, char **argv, char **col_name);
int verify_song_callback(void *controller, int argc, char **argv, char **col_name);
int bliss_library_callback(void *library, int argc, char **argv, char **col_name);
int bliss_analysis_callback(void *analysis, int argc, char **argv, char **col_name);
int get_song_id(db_song *db_song, sqlite3 *bliss_db, char *mpd_music_dir);
int random_playlist(struct mpd_connection *conn, sqlite3 *bliss_db, size_t playlist_len, char *music_dir);
void db_songs_free(size_t len, db_song *db_songs[len], size_t start);
int get_bliss_analysis_features(sqlite3* bliss_db, int song_id, bliss_analysis *analysis);
int get_bliss_library(sqlite3* bliss_db, int max_song_id, bliss_analysis *(*library)[max_song_id + 1]);

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
    fprintf(stderr, "Out of memory at replace_all malloc");
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
      fprintf(stderr, "Out of memory at replace_all realloc");
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
        fprintf(stderr, "Out of memory at replace_all if pos realloc");
        return NULL;
      }
      out = new_out;
      strncat(out, last, next_diff);
    }
    else {
      new_out = realloc(out, new_len + strlen(last));
      if (new_out == NULL) {
        free(out);
        fprintf(stderr, "Out of memory at replace_all else realloc");
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

void euclidean_distance_callback(void *center, va_alist alist) {
  bliss_analysis *center_ptr = center;
  va_start_ptr(alist, int);
  bliss_analysis **a_ptr = va_arg_ptr(alist, bliss_analysis**);
  if (a_ptr == NULL) { 
    fprintf(stderr, "Not enough arguments provided to euclidean_distance_callback: expected 2, got 0\n");
    exit(1);
  }
  bliss_analysis **b_ptr = va_arg_ptr(alist, bliss_analysis**);
  if (b_ptr == NULL) { 
    fprintf(stderr, "Not enough arguments provided to euclidean_distance_callback: expected 2, got 1\n");
    exit(1);
  }
  bliss_analysis *a = *a_ptr;
  bliss_analysis *b = *b_ptr;
  double distance_a = euclidean_distance(center_ptr->features, a->features, NUM_FEATURES);
  double distance_b = euclidean_distance(center_ptr->features, b->features, NUM_FEATURES);
  double diff = distance_a - distance_b;
  if (fabs(diff) < 0.05) {
    va_return_ptr(alist, int, 0);
  }
  else {
    va_return_ptr(alist, int, diff > 0 ? 1 : -1 );
  }
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

int song_id_callback(void *song_id, int argc, char **argv, char **col_name) {
  int *song_id_ptr = song_id;
  return !get_int_by_column_name("id", argc, argv, col_name, song_id_ptr);
}

int song_query_callback(void *song_ptr, int argc, char **argv, char **col_name) {
  db_song *song = song_ptr;
  int path_col_pos = str_pos_in_arr("path", col_name, argc);
  if (path_col_pos == -1) {
    fprintf(stderr, "path column not found at song_query_callback\n");
    return 1;
  }
  int len = strlen(argv[path_col_pos]) + 1;
  song->path = malloc(len);
  if (song->path == NULL) {
    fprintf(stderr, "Out of memory at song_query_callback malloc\n");
    return 1;
  }
  if (strcmp(argv[0], "29") == 0) {
    printf("debug");
  }
  strcpy(song->path, argv[path_col_pos]);
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
    fprintf(stderr, "Out of memory at get_song_id malloc");
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
    fprintf(stderr, "Out of memory at query_builder\n");
    exit(1);
  }
  snprintf(query, len, "%s%d;", header, variable);
  return query;
}

void db_songs_free(size_t len, db_song *songs[len], size_t start) {
  for (int i = start; i < len; i++) {
    if (songs[i] != NULL && songs[i]->path != NULL) {
      free(songs[i]->path);
      free(songs[i]);
    }
  }
}

int random_query_callback(void *controller_ptr, int argc, char **argv, char **col_name) {
  random_query_controller *controller = controller_ptr;
  db_song *song = malloc(sizeof(db_song));
  if (song == NULL) {
    fprintf(stderr, "Out of memory at random_query_callback song malloc\n");
    return 1;
  }
  int song_id = 0;
  if (!get_int_by_column_name("id", argc, argv, col_name, &song_id)) {
    free(song);
    return 1;
  }
  song->song_id = song_id;
  int path_col_pos = str_pos_in_arr("path", col_name, argc);
  if (path_col_pos == -1) {
    fprintf(stderr, "path column not found at random_query_callback\n");
    free(song);
    return 1;
  }
  song->path = malloc(strlen(argv[path_col_pos]) - strlen(controller->music_dir) + 1);
  if (song->path == NULL) {
    fprintf(stderr, "Out of memory at random_query_callback song->path malloc\n");
    free(song);
    return 1;
  }
  if (strstr(argv[path_col_pos], controller->music_dir) == argv[path_col_pos]) {
    strcpy(song->path, argv[path_col_pos] + strlen(controller->music_dir));
  }
  else {
    fprintf(stderr, "Failed to remove music_dir prefix at random_query_callback\n");
    return 1;
  }
  controller->songs[controller->i] = song;
  controller->i++;
  return 0;
}

int random_playlist(struct mpd_connection *conn, sqlite3 *bliss_db, size_t playlist_len, char *music_dir) {
  char *z_err = 0;
  srand((unsigned) time(NULL));
  printf("Creating random playlist of length %jd...\n", playlist_len);
  db_song *songs[playlist_len];
  random_query_controller controller = {.i = 0, .music_dir = music_dir, .songs = songs};
  char *random_query = query_builder("select * from song order by random() limit ", playlist_len, 2);
  int bliss_rc = sqlite3_exec(bliss_db, random_query, random_query_callback, &controller, &z_err);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite exec error at random_playlist: %s\n", z_err);
    free(random_query);
    free(z_err);
    return 1;
  }
  free(random_query);

  const char *random_playlist_name = "random";
  bool rm_rc = mpd_run_rm(conn, random_playlist_name);
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
    bool playlist_add_rc = mpd_run_playlist_add(conn, random_playlist_name, songs[i]->path);
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

int get_bliss_analysis_features(sqlite3* bliss_db, int song_id, bliss_analysis *analysis) {
  char *z_err = 0;
  char *bliss_analysis_query = query_builder("select * from feature where song_id = ", song_id, 2);
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

int get_bliss_library(sqlite3* bliss_db, int max_song_id, bliss_analysis *(*library)[max_song_id + 1]) {
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

int main(int argc, char **argv) {
  sqlite3 *bliss_db;
  struct mpd_connection *conn = mpd_connection_new(NULL, 0, 300000);
  if (conn == NULL) {
    fprintf(stderr, "Out of memory at mpd_connection_new\n");
    return 1;
  }
  mpd_connection_set_keepalive(conn, true);

  char *password = NULL;
  char *bliss_db_path = NULL;
  char *base_song_id = NULL;
  char *music_dir = NULL;

  while (1) {
    struct option long_opts[] = {
      { "password", required_argument, 0, 'p' },
      { "bliss-db", required_argument, 0, 'b' },
      { "song-id", required_argument, 0, 's' },
      { "music-dir", required_argument, 0, 'm' },
      { 0, 0, 0, 0 }
    };
    int opt_index = 0;
    int c = getopt_long(argc, argv, "p:b:s:m:", long_opts, &opt_index);
    if (c == -1) {
      break;
    }
    switch (c) {
      case 0:
        fprintf(stderr, "Unknown getopt_long option %c\n", c);
        break;
      case 'p':
        password = optarg;
        break;
      case 'b':
        bliss_db_path = optarg;
        break;
      case 's':
        base_song_id = optarg;
        break;
      case 'm':
        music_dir = optarg;
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

  if (password) {
    if (!mpd_run_password(conn, password)) {
      fprintf(stderr, "Bad password\n");
      mpd_connection_free(conn);
      return 1;
    }
  }

  if (bliss_db_path == NULL) {
    fprintf(stderr, "No bliss sqlite db location specified, use option --bliss-db\n");
    return 1;
  }

  if (music_dir == NULL) {
    fprintf(stderr, "No music directory specified, use option --music-dir\n");
    return 1;
  }

  int bliss_rc = sqlite3_open(bliss_db_path, &bliss_db);
  if (bliss_rc != SQLITE_OK) {
    fprintf(stderr, "sqlite error at open bliss_db: %s\n", sqlite3_errmsg(bliss_db));
    mpd_connection_free(conn);
    return 1;
  }

  if (mpd_connection_get_error(conn) != MPD_ERROR_SUCCESS) {
    fprintf(stderr, "mpd connection error at init: %s\n", mpd_connection_get_error_message(conn));
    mpd_connection_free(conn);
    int bliss_db_close_status = sqlite3_close(bliss_db);
    if (bliss_db_close_status != SQLITE_OK) {
      fprintf(stderr, "Failed to close bliss_db with error code %d\n", bliss_db_close_status);
    }
    return 1;
  }

  char blissify_cmd[256];
  if (password) {
    snprintf(blissify_cmd, 255, "MPD_HOST='%s@127.0.0.1' MPD_PORT=6600 blissify update", password); // TODO: get host and port programmatically
  }
  else {
    snprintf(blissify_cmd, 255, "MPD_PORT=6600 blissify update"); // TODO: get host and port programmatically
  }
  printf("Running `blissify update`...\n");
  int blissify_rc = system(blissify_cmd);
  if (blissify_rc != 0) {
    fprintf(stderr, "`blissify update` failed with exit code %d", blissify_rc);
    mpd_connection_free(conn);
    return 1;
  }

  if (base_song_id == NULL) {
    random_playlist(conn, bliss_db, 50, music_dir);
  }
  else {
    int *base_song_id_int = malloc(sizeof(int));
    if (base_song_id_int == NULL) {
      fprintf(stderr, "Out of memory at base_song_id_int malloc\n");
      return 1;
    } 
    strtol_err_wrap(base_song_id, base_song_id_int);
    printf("Making playlist from song_id %d\n", *base_song_id_int);
    int max_song_id = 0;
    if (!get_max_song_id(bliss_db, &max_song_id)) {
      fprintf(stderr, "Failed to get max song_id\n");
      return 1;
    }
    bliss_analysis *(*library)[max_song_id+1] = malloc(sizeof(bliss_analysis*[max_song_id+1]));
    if (library == NULL) {
      fprintf(stderr, "Out of memory at library malloc\n");
      return 1;
    }
    for (int i = 0; i <= max_song_id; i++) {
      bliss_analysis *analysis = calloc(1, sizeof(bliss_analysis));
      if (analysis == NULL) {
        fprintf(stderr, "Out of memory at analysis malloc for i %d", i);
        return 1;
      }
      (*library)[i] = analysis;
    }
    get_bliss_library(bliss_db, max_song_id, library);
    bliss_analysis *base_song_analysis = malloc(sizeof(bliss_analysis));
    base_song_analysis->song_id = *base_song_id_int;
    get_bliss_analysis_features(bliss_db, *base_song_id_int, base_song_analysis);
    callback_t sort_callback = alloc_callback(euclidean_distance_callback, base_song_analysis);
    qsort(*library, max_song_id + 1, sizeof(bliss_analysis*), (__compar_fn_t) sort_callback);
    for (int i = 0; i <= max_song_id; i++) {
      if ((*library)[i] != NULL) {
        printf("song_id %d\n", (*library)[i]->song_id);
        free((*library)[i]);
      }
    }
    free(library);
  }

  printf("Closing...\n");
  mpd_connection_free(conn);
  int bliss_db_close_status = sqlite3_close(bliss_db);
  if (bliss_db_close_status != SQLITE_OK) {
    fprintf(stderr, "Failed to close bliss_db with error code %d\n", bliss_db_close_status);
  }

  return 0;
}
