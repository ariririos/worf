#include "deps/dotenv-c/dotenv.h"
#include <assert.h>
#include <callback.h>
#include <errno.h>
#include <getopt.h>
#include <math.h>
#include <mpd/client.h>
#include <sqlite3.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
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
  db_song **songs;
  size_t length;
  size_t capacity;
} dynamic_songs_array;

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

typedef struct {
  char *music_dir;
  db_song *song;
} song_query_controller;

typedef struct {
  char *music_dir;
  dynamic_songs_array *songs;
} glob_query_controller;

bool int_in_arr(int needle, int *haystack, size_t len);
int str_pos_in_arr(char *needle, char **haystack, size_t len);
char *replace_all(char *str, const char *substr, const char *replacement);
double euclidean_distance(double *a, double *b, int dimensions);
int oom_message(char *loc, ...);
void maybe_free_music_dir(char *music_dir);
int init_dynamic_songs_array(dynamic_songs_array *arr, size_t init_size);
int push_dynamic_songs_array(dynamic_songs_array *arr, db_song *element);
void free_dynamic_songs_array(dynamic_songs_array *arr);
bool strtol_err_wrap(char *str, int *output);
bool get_int_by_column_name(char *col_name, int argc, char **argv, char **col_names, int *val);
void euclidean_distance_callback(void *center, va_alist alist);
int song_id_callback(void *song_id, int argc, char **argv, char **col_name);
int song_query_callback(void *controller_ptr, int argc, char **argv, char **col_name);
int bliss_library_callback(void *library, int argc, char **argv, char **col_name);
int bliss_analysis_callback(void *analysis, int argc, char **argv, char **col_name);
int get_song_id(db_song *db_song, sqlite3 *bliss_db, char *mpd_music_dir);
char *query_builder(char *header, int variable, int footer);
void db_songs_free(size_t len, db_song *db_songs[len], size_t start);
void library_songs_free(size_t len, bliss_analysis *(*library)[len], size_t start);
int populate_song(db_song *song, char *music_dir, int argc, char **argv, char **col_name);
int random_query_callback(void *controller_ptr, int argc, char **argv, char **col_name);
int glob_query_callback(void *controller_ptr, int argc, char **argv, char **col_name);
int random_playlist(struct mpd_connection *conn, sqlite3 *bliss_db, size_t playlist_len, char *music_dir, char *playlist_name);
int get_bliss_analysis_features(sqlite3 *bliss_db, int song_id, bliss_analysis *analysis);
int get_bliss_library(sqlite3 *bliss_db, int max_song_id, bliss_analysis *(*library)[max_song_id + 1]);
int max_song_id_callback(void *max_song_id, int argc, char **argv, char **col_name);
bool get_max_song_id(sqlite3 *bliss_db, int *max_song_id);
int playlist_from_song_id(sqlite3 *bliss_db, char *music_dir, int base_song_id, struct mpd_connection *conn, char *playlist_name, int playlist_len);